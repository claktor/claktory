package com.claktory.app

import android.os.ParcelFileDescriptor
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.nio.ByteBuffer

/**
 * factory/device's appVersionCode: the installed build's versionCode, the
 * one Android compares on an update, as the package manager's own dump
 * says it (the Android update notice compares it with claktory.com's).
 */
@RunWith(AndroidJUnit4::class)
class AppVersionCodeTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    private object Nowhere : BinaryMessenger {
        override fun send(channel: String, message: ByteBuffer?) {}

        override fun send(channel: String, message: ByteBuffer?, callback: BinaryMessenger.BinaryReply?) {}

        override fun setMessageHandler(channel: String, handler: BinaryMessenger.BinaryMessageHandler?) {}
    }

    /** versionCode=… from `dumpsys package`, read apart from the app. */
    private fun dumped(): Long {
        val out = instrumentation.uiAutomation.executeShellCommand("dumpsys package ${context.packageName}")
        val text = ParcelFileDescriptor.AutoCloseInputStream(out).bufferedReader().use { it.readText() }
        val code = Regex("""versionCode=(\d+)""").find(text) ?: throw AssertionError("dumpsys has no versionCode")
        return code.groupValues[1].toLong()
    }

    @Test
    fun answersTheInstalledVersionCode() {
        var answer: Any? = null
        val channel = DeviceChannel(Nowhere, context)
        channel.onMethodCall(MethodCall("appVersionCode", null), object : MethodChannel.Result {
            override fun success(result: Any?) { answer = result }
            override fun error(code: String, message: String?, details: Any?) = throw AssertionError("appVersionCode: $code $message")
            override fun notImplemented() = throw AssertionError("appVersionCode isn't implemented")
        })
        channel.close()
        // A Long, which Dart reads as an int.
        assertTrue("a Long, not ${answer?.javaClass}", answer is Long)
        assertEquals(dumped(), answer)
        assertEquals(dumped(), DeviceChannel.appVersionCode(context))
    }
}
