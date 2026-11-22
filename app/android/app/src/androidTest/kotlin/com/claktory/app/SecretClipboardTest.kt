package com.claktory.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
}
