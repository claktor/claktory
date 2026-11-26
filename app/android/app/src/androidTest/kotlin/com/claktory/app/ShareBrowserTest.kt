package com.claktory.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.net.InetAddress
import java.net.ServerSocket
import java.util.Collections
import kotlin.concurrent.thread

/**
 * A shared website opened on the phone (W6): in a Custom Tab with a session
 * that binds ShareKeepAlive while it's on screen, at the share's own
 * s-….localhost host, served from this process's loopback as the forward
 * serves it. Needs a browser that takes Custom Tabs (Chrome on the
 * emulator), past its first-run screens.
 */
@RunWith(AndroidJUnit4::class)
class ShareBrowserTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    private fun waitFor(what: String, seconds: Int = 20, check: () -> Boolean) {
        val until = System.currentTimeMillis() + seconds * 1000L
        while (System.currentTimeMillis() < until) {
            if (check()) return
            Thread.sleep(100)
        }
        throw AssertionError("no $what within $seconds s")
    }

    private fun call(screen: Screen, method: String, arguments: Any?): List<Any?> {
        val answers = Collections.synchronizedList(mutableListOf<Any?>())
        screen.onActivity {
            it.adapter(ShareBrowser::class.java)!!.onMethodCall(
                MethodCall(method, arguments),
                object : MethodChannel.Result {
                    override fun success(result: Any?) {
                        answers += result
                    }

                    override fun error(code: String, message: String?, details: Any?) {
                        answers += "error $code"
                    }

                    override fun notImplemented() {
                        answers += "notImplemented"
                    }
                },
            )
        }
        waitFor("the answer") { answers.isNotEmpty() }
        return answers.toList()
    }

    @Test
    fun aSharesPageOpensInACustomTabThatHoldsTheAppUp() {
        assumeTrue("a browser that takes Custom Tabs", androidx.browser.customtabs.CustomTabsClient.getPackageName(context, null) != null)
        val hosts = Collections.synchronizedList(mutableListOf<String>())
        val server = ServerSocket(0, 10, InetAddress.getByName("127.0.0.1"))
        thread(isDaemon = true) {
            while (true) {
                val socket = runCatching { server.accept() }.getOrNull() ?: break
                socket.use {
                    val input = it.getInputStream().bufferedReader()
                    var line = input.readLine()
                    while (!line.isNullOrEmpty()) {
                        if (line.startsWith("Host:", ignoreCase = true)) hosts += line.substringAfter(':').trim()
                        line = input.readLine()
                    }
                    it.getOutputStream().write("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".toByteArray())
                }
            }
        }
        try {
            Screen.launch().use { screen ->
                val address = "s-test.localhost:${server.localPort}"
                // Never a bare loopback address.
                assertEquals(listOf<Any?>("error bad_url"), call(screen, "open", mapOf("url" to "http://127.0.0.1:${server.localPort}/")))
                assertEquals(listOf<Any?>("custom_tab"), call(screen, "open", mapOf("url" to "http://$address/")))
                waitFor("the page asked for at its own host") { hosts.contains(address) }
                waitFor("the tab holding the app up") { ShareKeepAlive.visible }
                instrumentation.shell("input keyevent KEYCODE_BACK")
                waitFor("the tab gone") { !ShareKeepAlive.visible }
            }
        } finally {
            server.close()
        }
        assertTrue(hosts.all { it.startsWith("s-test.localhost:") })
    }
}
