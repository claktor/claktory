package com.claktory.app

import android.Manifest
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.BinaryMessenger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.nio.ByteBuffer

/**
 * The pairing screen's camera on a phone or emulator: the permission as
 * factory/device answers it, and a scanner view that opens the camera,
 * reads its frames, closes it when the app leaves the screen and opens it
 * again on return. The emulator's camera shows no QR code: reading one
 * is [QrCodesTest]'s, and a real code on a real phone is checked by hand.
 */
@RunWith(AndroidJUnit4::class)
class QrScannerTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    /** Channels that go nowhere: the view's "scanned" isn't what's tested here. */
    private object Nowhere : BinaryMessenger {
        override fun send(channel: String, message: ByteBuffer?) {}

        override fun send(channel: String, message: ByteBuffer?, callback: BinaryMessenger.BinaryReply?) {}

        override fun setMessageHandler(channel: String, handler: BinaryMessenger.BinaryMessageHandler?) {}
    }

    private fun waitFor(what: String, seconds: Int = 15, check: () -> Boolean) {
        val until = System.currentTimeMillis() + seconds * 1000L
        while (System.currentTimeMillis() < until) {
            if (check()) return
            Thread.sleep(100)
        }
        throw AssertionError("no $what within $seconds s")
    }

    @Test
    fun theCameraOpensReadsFramesAndClosesOffScreen() {
        grant(Manifest.permission.CAMERA)
        Screen.launch().use { scenario ->
            lateinit var view: ScannerView
            val factory = ScannerFactory(Nowhere)
            scenario.onActivity { activity ->
                assertEquals("granted", CameraAccess(activity).access())
                view = factory.create(activity, 9_001, null) as ScannerView
                activity.addContentView(
                    FrameLayout(activity).apply { addView(view.view, ViewGroup.LayoutParams(480, 800)) },
                    ViewGroup.LayoutParams(480, 800),
                )
            }
            waitFor("frames from the camera") { view.framesRead >= 3 }

            scenario.onActivity { factory.onPause() }
            Thread.sleep(500)
            val shut = view.framesRead
            Thread.sleep(1_000)
            assertEquals("no frames once the camera is closed", shut, view.framesRead)

            scenario.onActivity { factory.onResume() }
            waitFor("frames after coming back") { view.framesRead >= shut + 3 }

            scenario.onActivity { factory.close() }
            Thread.sleep(500)
            val closed = view.framesRead
            Thread.sleep(1_000)
            assertEquals(closed, view.framesRead)
        }
    }

    @Test
    fun aPairingCodeMakesAQrCodeAsTheIphoneAnswersIt() {
        val code = QrCodes.modules("claktory://pair?k=abc&a=node.example:7420")!!
        val size = code["size"] as Int
        assertTrue(size >= 21)
        assertEquals(size * size, (code["modules"] as ByteArray).size)
    }
}
