package com.claktory.app

import android.app.Instrumentation
import android.content.Intent
import android.content.IntentFilter
import android.os.ParcelFileDescriptor
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.nio.ByteBuffer

/**
 * factory/device's appSource and openAppSource (Settings' This phone,
 * ST1): Claktory Dev, which these tests run as, comes from nowhere a
 * person updates it; Play's listing opens for "google_play" and nothing
 * for anything else. Opening is caught before anything starts.
 */
@RunWith(AndroidJUnit4::class)
class AppSourceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    private object Nowhere : BinaryMessenger {
        override fun send(channel: String, message: ByteBuffer?) {}

        override fun send(channel: String, message: ByteBuffer?, callback: BinaryMessenger.BinaryReply?) {}

        override fun setMessageHandler(channel: String, handler: BinaryMessenger.BinaryMessageHandler?) {}
    }

    private fun call(method: String, arguments: Any?): Any? {
        var answer: Any? = "unanswered"
        val channel = DeviceChannel(Nowhere, context)
        channel.onMethodCall(MethodCall(method, arguments), object : MethodChannel.Result {
            override fun success(result: Any?) { answer = result }
            override fun error(code: String, message: String?, details: Any?) = throw AssertionError("$method: $code $message")
            override fun notImplemented() = throw AssertionError("$method isn't implemented")
        })
        channel.close()
        return answer
    }

    /** Catches every ACTION_VIEW the app starts, so nothing opens. */
    private fun <T> catchingViews(body: (Instrumentation.ActivityMonitor) -> T): T {
        val filter = IntentFilter(Intent.ACTION_VIEW).apply {
            addCategory(Intent.CATEGORY_DEFAULT)
            addDataScheme("market")
            addDataScheme("https")
        }
        val monitor = instrumentation.addMonitor(filter, null, true)
        try {
            return body(monitor)
        } finally {
            instrumentation.removeMonitor(monitor)
        }
    }

    /** installerPackageName=… from `dumpsys package`, read apart from the app; null for "null". */
    private fun dumpedInstaller(): String? {
        val out = instrumentation.uiAutomation.executeShellCommand("dumpsys package ${context.packageName}")
        val text = ParcelFileDescriptor.AutoCloseInputStream(out).bufferedReader().use { it.readText() }
        val found = Regex("""installerPackageName=(\S+)""").find(text) ?: throw AssertionError("dumpsys has no installer")
        return found.groupValues[1].takeUnless { it == "null" }
    }

    /** Whatever installed it (adb, or `adb install -i com.android.vending` to stand in for Play). */
    @Test
    fun readsTheInstallerTheSystemRecorded() {
        assertEquals(dumpedInstaller(), AppSource.installer(context))
    }

    @Test
    fun claktoryDevComesFromNowhere() {
        assertTrue(context.packageName.endsWith(".dev"))
        assertNull(call("appSource", null))
    }

    @Test
    fun googlePlayOpensItsListing() {
        catchingViews { monitor ->
            assertEquals(true, call("openAppSource", "google_play"))
            assertEquals(1, monitor.hits)
        }
    }

    @Test
    fun anythingElseOpensNothing() {
        catchingViews { monitor ->
            for (source in listOf("website", "app_store", "none", null, 3)) {
                assertEquals(false, call("openAppSource", source))
            }
            assertEquals(0, monitor.hits)
        }
    }

    @Test
    fun theListingIsPlaysThenTheWeb() {
        val (app, web) = AppSource.listingIntents("com.claktory.app")
        assertEquals("market://details?id=com.claktory.app", app.dataString)
        assertEquals(AppSource.PLAY, app.`package`)
        assertEquals("https://play.google.com/store/apps/details?id=com.claktory.app", web.dataString)
        assertNull(web.`package`)
        assertEquals(Intent.ACTION_VIEW, app.action)
        assertEquals(Intent.ACTION_VIEW, web.action)
    }
}
