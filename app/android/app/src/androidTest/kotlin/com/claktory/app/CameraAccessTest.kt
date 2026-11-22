package com.claktory.app

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.MethodChannel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.util.Collections

/**
 * cameraAccess tells "refused once, Android may still ask" (undetermined:
 * the pairing screen asks again) from "Android won't ask again" (denied:
 * only Settings), with nothing kept on the device: the system's own
 * permission flags are set as a person's answers would set them. Runs
 * before anything grants the camera (revoking it would end this process),
 * so it's skipped where the camera is already allowed.
 */
@RunWith(AndroidJUnit4::class)
class CameraAccessTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    private fun shell(command: String): String =
        instrumentation.uiAutomation.executeShellCommand(command).use { pipe ->
            java.io.FileInputStream(pipe.fileDescriptor).use { String(it.readBytes()) }
        }

    /**
     * Sets the camera's permission flags as a person's answers would. Older
     * Androids have no `pm set-permission-flags`: the test skips there
     * rather than ask for real and leave Android's question up.
     */
    private fun flags(set: String) {
        // Its "unknown command" goes to stderr, which this can't read, so
        // the version decides.
        assumeTrue("pm set-permission-flags (Android 11+)", Build.VERSION.SDK_INT >= Build.VERSION_CODES.R)
        shell("pm set-permission-flags ${context.packageName} ${Manifest.permission.CAMERA} $set")
    }

    @After
    fun tearDown() {
        shell("pm clear-permission-flags ${context.packageName} ${Manifest.permission.CAMERA} user-set user-fixed")
    }

    private fun waitFor(what: String, seconds: Int = 10, check: () -> Boolean) {
        val until = System.currentTimeMillis() + seconds * 1000L
        while (System.currentTimeMillis() < until) {
            if (check()) return
            Thread.sleep(100)
        }
        throw AssertionError("no $what within $seconds s")
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

    private fun notYetAllowed() = assumeTrue(
        "the camera isn't allowed yet",
        context.checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED,
    )

    @Test
    fun refusedForGoodIsDeniedOnlyOnceAskingSaysSo() {
        notYetAllowed()
        flags("user-set user-fixed")
        Screen.launch().use { screen ->
            lateinit var access: CameraAccess
            screen.onActivity { access = it.adapter(CameraAccess::class.java)!! }
            var before = ""
            screen.onActivity { before = access.access() }
            assertEquals("not known until asked", "undetermined", before)
            val answer = Answer()
            screen.onActivity { access.request(answer) }
            // No question shows: Android answers no at once.
            waitFor("Android's answer") { answer.values.isNotEmpty() }
            assertEquals(listOf<Any?>(false), answer.values.toList())
            var after = ""
            screen.onActivity { after = access.access() }
            assertEquals("denied", after)
        }
    }

    @Test
    fun refusedOnceIsStillUndetermined() {
        notYetAllowed()
        // A person's one "Don't allow": Android may ask once more.
        flags("user-set")
        Screen.launch().use { screen ->
            lateinit var access: CameraAccess
            var rationale = false
            screen.onActivity {
                access = it.adapter(CameraAccess::class.java)!!
                rationale = it.shouldShowRequestPermissionRationale(Manifest.permission.CAMERA)
            }
            assumeTrue("this Android reads a refusal from the flag", rationale)
            var now = ""
            screen.onActivity {
                // The refusal as Android hands it to the app.
                access.onPermissionResult(
                    CameraAccess.REQUEST,
                    arrayOf(Manifest.permission.CAMERA),
                    intArrayOf(PackageManager.PERMISSION_DENIED),
                )
                now = access.access()
            }
            assertEquals("undetermined", now)
        }
    }
}
