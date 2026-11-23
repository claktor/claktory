package com.claktory.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * factory/secret-clipboard with Android's own clipboard, from the running
 * app (Android lets only the app in front read what's there): the code
 * marked sensitive, cleared while it's still there, and what was copied
 * since left alone.
 */
@RunWith(AndroidJUnit4::class)
class SecretClipboardTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val clipboard get() = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    private val code = "claktory://add?code=INSTRUMENTATION-HANDOFF"

    private fun focused(screen: Screen) {
        val until = System.currentTimeMillis() + 5_000
        while (System.currentTimeMillis() < until) {
            var focused = false
            screen.onActivity { focused = it.hasWindowFocus() }
            if (focused) return
            Thread.sleep(50)
        }
        throw AssertionError("the app's screen never had focus")
    }

    private fun text(): String? = clipboard.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()

    @Test
    fun theCodeIsSensitiveAndClearedOnlyWhileStillThere() {
        Screen.launch().use { screen ->
            focused(screen)
            var channel: SecretClipboardChannel? = null
            screen.onActivity { channel = it.adapter(SecretClipboardChannel::class.java) }
            val core = (channel ?: throw AssertionError("factory/secret-clipboard isn't registered")).core

            screen.onActivity {
                val token = core.copy(code)
                assertEquals(code, text())
                val description = clipboard.primaryClipDescription!!
                assertEquals(SecretClipboardCore.LABEL, description.label.toString())
                assertTrue(description.extras!!.getBoolean(SecretClipboardCore.EXTRA_IS_SENSITIVE))
                assertTrue(core.clear(token))
                val left = text()
                assertTrue("cleared, not '$left'", left.isNullOrEmpty())

                // Copied since, as Flutter's own Copy does: left alone.
                val later = core.copy(code)
                clipboard.setPrimaryClip(ClipData.newPlainText("text label?", "a message they copied"))
                assertFalse(core.clear(later))
                assertEquals("a message they copied", text())

                // Copy code twice: the first token never clears the second.
                val first = core.copy(code)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    // The system's clock may give both the same millisecond.
                    Thread.sleep(5)
                    val second = core.copy(code)
                    assertFalse(core.clear(first))
                    assertEquals(code, text())
                    assertTrue(core.clear(second))
                } else {
                    assertTrue(core.clear(first))
                }
                assertTrue(text().isNullOrEmpty())
            }
        }
    }

    @Test
    fun sweepClearsAnExpiredCodeAndLeavesTheRestAlone() {
        Screen.launch().use { screen ->
            focused(screen)
            screen.onActivity {
                var now = System.currentTimeMillis()
                // A core of its own, on Android's clipboard: as after the app was stopped, no token.
                val core = SecretClipboardCore(AndroidClipboard(context)) { now }

                // Not yet expired: left alone.
                core.copy(code, seconds = 600)
                val description = clipboard.primaryClipDescription!!
                assertEquals(now + 600_000, description.extras!!.getLong(SecretClipboardCore.EXTRA_EXPIRES_MS))
                assertTrue(description.extras!!.getBoolean(SecretClipboardCore.EXTRA_IS_SENSITIVE))
                val expiresAt = now + 600_000
                now += 599_000
                assertEquals(expiresAt, core.sweep())
                assertEquals(code, text())

                // Expired: cleared.
                now += 1_000
                assertNull(core.sweep())
                val left = text()
                assertTrue("cleared, not '$left'", left.isNullOrEmpty())

                // Another label, even with our extra and long past it: left alone.
                val other = ClipData.newPlainText("text label?", "a message they copied")
                other.description.extras = android.os.PersistableBundle().apply {
                    putLong(SecretClipboardCore.EXTRA_EXPIRES_MS, now - 1)
                }
                clipboard.setPrimaryClip(other)
                assertNull(core.sweep())
                assertEquals("a message they copied", text())

                // Ours with no expiry (copied without seconds): left alone.
                core.copy(code)
                now += 100_000_000
                assertNull(core.sweep())
                assertEquals(code, text())
                core.clear(core.copy(code))
            }
        }
    }

    /** Calls the channel's method as Flutter would, and answers what it sent back. */
    private fun call(channel: SecretClipboardChannel, method: String, arguments: Map<String, Any>? = null): Any? {
        var answer: Any? = null
        channel.onMethodCall(io.flutter.plugin.common.MethodCall(method, arguments), object : io.flutter.plugin.common.MethodChannel.Result {
            override fun success(result: Any?) { answer = result }
            override fun error(code: String, message: String?, details: Any?) = throw AssertionError("$method: $code $message")
            override fun notImplemented() = throw AssertionError("$method isn't implemented")
        })
        return answer
    }

    @Test
    fun copyAndSweepThroughTheChannel() {
        Screen.launch().use { screen ->
            focused(screen)
            var channel: SecretClipboardChannel? = null
            screen.onActivity { channel = it.adapter(SecretClipboardChannel::class.java) }
            val registered = channel ?: throw AssertionError("factory/secret-clipboard isn't registered")
            screen.onActivity {
                // copy's seconds land in the clip as its expiry, and sweep answers it.
                val before = System.currentTimeMillis()
                call(registered, "copy", mapOf("text" to code, "seconds" to 2))
                val expiresAt = clipboard.primaryClipDescription!!.extras!!.getLong(SecretClipboardCore.EXTRA_EXPIRES_MS)
                assertTrue(expiresAt in before + 2_000..System.currentTimeMillis() + 2_000)
                assertEquals(expiresAt, call(registered, "sweep"))
                assertTrue("a sweep waits for it", registered.scheduled != null)
                assertEquals(code, text())
            }
            // Past it, in front all along and with no sweep asked for: the host's own sweep cleared it.
            Thread.sleep(2_600)
            screen.onActivity {
                val left = text()
                assertTrue("cleared, not '$left'", left.isNullOrEmpty())
                assertNull(call(registered, "sweep"))
            }
        }
    }

    @Test
    fun aCodeExpiringAfterARelaunchGoesWhileTheAppStaysInFront() {
        // Row 185: copied before the app was stopped, still unexpired when it comes back.
        // Long enough ahead for a cold launch; the wait below runs to it from the clock.
        val expiresAt = System.currentTimeMillis() + 10_000
        val pending = ClipData.newPlainText(SecretClipboardCore.LABEL, code)
        pending.description.extras = android.os.PersistableBundle().apply {
            putBoolean(SecretClipboardCore.EXTRA_IS_SENSITIVE, true)
            putLong(SecretClipboardCore.EXTRA_EXPIRES_MS, expiresAt)
        }
        Screen.launch().use { screen ->
            focused(screen)
            screen.onActivity { clipboard.setPrimaryClip(pending) }
        }
        Screen.launch().use { screen ->
            focused(screen)
            // The launch's sweep leaves it: not yet expired.
            var left: String? = null
            screen.onActivity { left = text() }
            assertEquals(code, left)
            // The app stays in front past the expiry, and nothing asks: it goes all the same.
            assertTrue("relaunched too slowly to test this", System.currentTimeMillis() < expiresAt)
            Thread.sleep(expiresAt + 600 - System.currentTimeMillis())
            screen.onActivity { left = text() }
            assertTrue("cleared, not '$left'", left.isNullOrEmpty())
        }
    }

    @Test
    fun aForgedFarExpiryIsLeftAloneAndWaitsOnNothing() {
        // Fugo, row 185: another app's clip with our label and an expiry of Long.MAX_VALUE
        // once made the host's sweep post itself again at once, a busy loop on the main thread.
        val forged = ClipData.newPlainText(SecretClipboardCore.LABEL, "forged")
        forged.description.extras = android.os.PersistableBundle().apply {
            putLong(SecretClipboardCore.EXTRA_EXPIRES_MS, Long.MAX_VALUE)
        }
        Screen.launch().use { screen ->
            focused(screen)
            var channel: SecretClipboardChannel? = null
            screen.onActivity { channel = it.adapter(SecretClipboardChannel::class.java) }
            val registered = channel ?: throw AssertionError("factory/secret-clipboard isn't registered")
            screen.onActivity {
                clipboard.setPrimaryClip(forged)
                assertNull(call(registered, "sweep"))
                assertNull(registered.scheduled)
                assertEquals("forged", text())
            }
            // The main thread is free: a later step on it runs.
            Thread.sleep(300)
            screen.onActivity {
                assertNull(registered.scheduled)
                assertEquals("forged", text())
                clipboard.setPrimaryClip(ClipData.newPlainText("", ""))
            }
        }
    }

    @Test
    fun anExpiredCodeIsGoneWhenTheAppComesBack() {
        // Copied with an expiry already past, as if the app had been stopped since.
        val expired = ClipData.newPlainText(SecretClipboardCore.LABEL, code)
        expired.description.extras = android.os.PersistableBundle().apply {
            putBoolean(SecretClipboardCore.EXTRA_IS_SENSITIVE, true)
            putLong(SecretClipboardCore.EXTRA_EXPIRES_MS, System.currentTimeMillis() - 1)
        }
        Screen.launch().use { screen ->
            focused(screen)
            screen.onActivity { clipboard.setPrimaryClip(expired) }
        }
        // Launched again: the host sweeps once its window has focus, before the app asks.
        Screen.launch().use { screen ->
            focused(screen)
            var left: String? = "unread"
            screen.onActivity { left = text() }
            assertTrue("cleared, not '$left'", left.isNullOrEmpty())
        }
    }
}
