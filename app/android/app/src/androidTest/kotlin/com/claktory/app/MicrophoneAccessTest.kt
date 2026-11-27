package com.claktory.app

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.util.Collections

/**
 * factory/recorder's access, as the microphone's popover and the
 * permission sheet read it (area 5a): refused once, Android may still ask
 * (undetermined); refused for good, only Settings (denied), with nothing
 * kept on the device. Android's own permission flags are set as a
 * person's answers would set them. Runs before VoiceRecorderTest grants
 * the microphone (revoking it would end this process), and skips where
 * it's already allowed.
 */
@RunWith(AndroidJUnit4::class)
class MicrophoneAccessTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val mic = Manifest.permission.RECORD_AUDIO

    private fun shell(command: String): String =
        instrumentation.uiAutomation.executeShellCommand(command).use { pipe ->
            java.io.FileInputStream(pipe.fileDescriptor).use { String(it.readBytes()) }
        }

    private fun flags(set: String) {
        assumeTrue("pm set-permission-flags (Android 11+)", Build.VERSION.SDK_INT >= Build.VERSION_CODES.R)
        shell("pm set-permission-flags ${context.packageName} $mic $set")
    }

    @Before
    fun setUp() {
        assumeTrue(
            "the microphone isn't allowed yet",
            context.checkSelfPermission(mic) != PackageManager.PERMISSION_GRANTED,
        )
        Refusals.forget()
    }

    @After
    fun tearDown() {
        Refusals.forget()
        shell("pm clear-permission-flags ${context.packageName} $mic user-set user-fixed")
    }

    private class Answer : MethodChannel.Result {
        val values: MutableList<Any?> = Collections.synchronizedList(mutableListOf())

        override fun success(result: Any?) {
            values += result
        }

        override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
            values += "error $errorCode"
        }

        override fun notImplemented() {
            values += "notImplemented"
        }
    }

    private fun call(screen: Screen, method: String): Any? {
        val answer = Answer()
        screen.onActivity { it.adapter(RecorderChannel::class.java)!!.onMethodCall(MethodCall(method, null), answer) }
        val until = System.currentTimeMillis() + 10_000
        while (answer.values.isEmpty() && System.currentTimeMillis() < until) Thread.sleep(100)
        return answer.values.singleOrNull() ?: throw AssertionError("no answer to $method within 10 s")
    }

    @Test
    fun refusedForGoodIsDeniedOnlyOnceAskingSaysSo() {
        flags("user-set user-fixed")
        Screen.launch().use { screen ->
            assertEquals("not known until asked", "undetermined", call(screen, "access"))
            assertEquals(false, call(screen, "refusedOnce"))
            // No question shows: Android answers no at once.
            assertEquals(false, call(screen, "request"))
            assertEquals("denied", call(screen, "access"))
        }
        // The next activity (a turn of the phone) still knows.
        Screen.launch().use { screen -> assertEquals("denied", call(screen, "access")) }
    }

    @Test
    fun refusedOnceIsStillUndetermined() {
        flags("user-set")
        Screen.launch().use { screen ->
            var rationale = false
            screen.onActivity { rationale = it.shouldShowRequestPermissionRationale(mic) }
            assumeTrue("this Android reads a refusal from the flag", rationale)
            screen.onActivity {
                it.adapter(RecorderChannel::class.java)!!.onPermissionResult(
                    RecorderChannel.REQUEST,
                    arrayOf(mic),
                    intArrayOf(PackageManager.PERMISSION_DENIED),
                )
            }
            assertEquals("undetermined", call(screen, "access"))
            assertEquals("the row's Off with Turn on…", true, call(screen, "refusedOnce"))
        }
    }

    @Test
    fun aQuestionSetAsideWithBackIsStillUndetermined() {
        // Never asked: Android's question shows. Back sets it aside, and
        // Android asks again next time, so the sheet's Before comes back
        // rather than Declined.
        assumeTrue("Android 11+ reads the flags this clears", Build.VERSION.SDK_INT >= Build.VERSION_CODES.R)
        Screen.launch().use { screen ->
            val answer = Answer()
            screen.onActivity { it.adapter(RecorderChannel::class.java)!!.onMethodCall(MethodCall("request", null), answer) }
            Thread.sleep(PermissionState.UNSEEN_MS + 1_000)
            assertEquals("Android's question is still up", 0, answer.values.size)
            shell("input keyevent KEYCODE_BACK")
            val until = System.currentTimeMillis() + 10_000
            while (answer.values.isEmpty() && System.currentTimeMillis() < until) Thread.sleep(100)
            assertEquals(listOf<Any?>(false), answer.values.toList())
            assertEquals("undetermined", call(screen, "access"))
        }
    }
}
