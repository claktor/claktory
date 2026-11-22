package com.claktory.app

import android.graphics.Bitmap
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.view.InputDevice
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.io.FileOutputStream

/**
 * A key chord as a keyboard sends it: each modifier pressed in turn,
 * carrying its own meta state and the ones before it, the key pressed and
 * released with all of them, then the modifiers released in reverse.
 */
class KeyChord(val modifiers: List<Int>, val key: Int) {
    /** The events, in order: (action, key code, meta state). */
    fun events(): List<Triple<Int, Int, Int>> {
        val events = mutableListOf<Triple<Int, Int, Int>>()
        var meta = 0
        for (modifier in modifiers) {
            meta = meta or META.getValue(modifier)
            events += Triple(KeyEvent.ACTION_DOWN, modifier, meta)
        }
        events += Triple(KeyEvent.ACTION_DOWN, key, meta)
        events += Triple(KeyEvent.ACTION_UP, key, meta)
        for (modifier in modifiers.reversed()) {
            meta = meta and META.getValue(modifier).inv()
            events += Triple(KeyEvent.ACTION_UP, modifier, meta)
        }
        return events
    }

    companion object {
        private val MODIFIERS = mapOf(
            "ctrl" to KeyEvent.KEYCODE_CTRL_LEFT,
            "shift" to KeyEvent.KEYCODE_SHIFT_LEFT,
            "alt" to KeyEvent.KEYCODE_ALT_LEFT,
            "meta" to KeyEvent.KEYCODE_META_LEFT,
        )
        private val META = mapOf(
            KeyEvent.KEYCODE_CTRL_LEFT to (KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON),
            KeyEvent.KEYCODE_SHIFT_LEFT to (KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON),
            KeyEvent.KEYCODE_ALT_LEFT to (KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON),
            KeyEvent.KEYCODE_META_LEFT to (KeyEvent.META_META_ON or KeyEvent.META_META_LEFT_ON),
        )
        private val NAMED = mapOf(
            "enter" to KeyEvent.KEYCODE_ENTER,
            "escape" to KeyEvent.KEYCODE_ESCAPE,
            "esc" to KeyEvent.KEYCODE_ESCAPE,
            "tab" to KeyEvent.KEYCODE_TAB,
            "space" to KeyEvent.KEYCODE_SPACE,
            "backspace" to KeyEvent.KEYCODE_DEL,
            "delete" to KeyEvent.KEYCODE_FORWARD_DEL,
            "up" to KeyEvent.KEYCODE_DPAD_UP,
            "down" to KeyEvent.KEYCODE_DPAD_DOWN,
            "left" to KeyEvent.KEYCODE_DPAD_LEFT,
            "right" to KeyEvent.KEYCODE_DPAD_RIGHT,
            "home" to KeyEvent.KEYCODE_MOVE_HOME,
            "end" to KeyEvent.KEYCODE_MOVE_END,
            "pageup" to KeyEvent.KEYCODE_PAGE_UP,
            "pagedown" to KeyEvent.KEYCODE_PAGE_DOWN,
        )

        /** "ctrl+f", "ctrl+shift+g", "escape", "enter": modifiers, then one key. */
        fun parse(text: String): KeyChord {
            val parts = text.lowercase().split("+").map { it.trim() }
            require(parts.isNotEmpty() && parts.all { it.isNotEmpty() }) { "not a chord: $text" }
            val modifiers = parts.dropLast(1).map { MODIFIERS[it] ?: throw IllegalArgumentException("not a modifier: $it") }
            require(modifiers.toSet().size == modifiers.size) { "a modifier twice: $text" }
            val last = parts.last()
            val key = when {
                last.length == 1 && last[0] in 'a'..'z' -> KeyEvent.KEYCODE_A + (last[0] - 'a')
                last.length == 1 && last[0] in '0'..'9' -> KeyEvent.KEYCODE_0 + (last[0] - '0')
                last.matches(Regex("f([1-9]|1[0-2])")) -> KeyEvent.KEYCODE_F1 + last.drop(1).toInt() - 1
                else -> NAMED[last] ?: throw IllegalArgumentException("not a key: $last")
            }
            return KeyChord(modifiers, key)
        }
    }
}

/**
 * Not a test: a fixture for Android shortcut gates (Doppio's), sending a
 * key chord to the app as a hardware keyboard would. `adb shell input
 * keycombination` can't: its Ctrl down carries no Ctrl meta state, so
 * Flutter releases Ctrl at once (DESIGN.md §5, "Keyboard shortcuts and
 * focus"). Skipped unless asked for:
 *
 *     adb shell am instrument -w -e class com.claktory.app.KeyboardShortcutFixture \
 *       -e keys ctrl+f [-e tap 540,1150] [-e wait 10] [-e hold 20] \
 *       [-e expect "'Done|Next match|Search this conversation'"] \
 *       com.claktory.app.dev.test/androidx.test.runner.AndroidJUnitRunner
 *
 * Instrumentation restarts the app, so this opens its screen (on what the
 * backend shows: the same conversation), waits `wait` seconds (8 by
 * default) for it to load, taps at `tap` first if given (touch mode, as a
 * person's tap leaves it), sends `keys` (several separated by commas, a
 * second apart), and keeps the screen up `hold` seconds (3 by default) to
 * look at with screencap or uiautomator. It saves a screenshot after the
 * keys in the app's cache (status `fixture_screenshot`: `adb shell run-as
 * com.claktory.app.dev cat cache/<name> > shot.png`), and reports each event
 * sent and where focus was. With `expect` (labels separated by |, quoted
 * twice as above: adb hands the command to the phone's shell, where a bare
 * | would start a pipeline), it then
 * waits up to `expect_wait` seconds (5 by default) for each label on
 * screen, read from the app's accessibility tree through this fixture's
 * own UiAutomation (uiautomator can't connect while it runs): a node whose
 * text, description, hint or tooltip (or one line of it) is exactly
 * the label. It reports how
 * many of each it found (`fixture_expect`), never any other text, and
 * fails unless every label was found. When the instrumentation ends,
 * Android stops the app's process; what it kept (pairing, drafts) stays.
 */
@RunWith(AndroidJUnit4::class)
class KeyboardShortcutFixture {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val arguments: Bundle = InstrumentationRegistry.getArguments()

    @Test
    fun sendsAChordAsAKeyboardWould() {
        val keys = arguments.getString("keys")
        assumeTrue("only when asked for, with -e keys", keys != null)
        val chords = keys!!.split(",").map { KeyChord.parse(it) }
        val wait = arguments.getString("wait")?.toLong() ?: 8
        val hold = arguments.getString("hold")?.toLong() ?: 3
        val tap = arguments.getString("tap")?.split(",")?.map { it.trim().toFloat() }
        require(tap == null || tap.size == 2) { "tap is X,Y" }
        Screen.launch().use { screen ->
            Thread.sleep(wait * 1000)
            if (tap != null) {
                tap(tap[0], tap[1])
                Thread.sleep(800)
            }
            report("before", screen)
            for ((index, chord) in chords.withIndex()) {
                if (index > 0) Thread.sleep(1000)
                send(chord)
            }
            Thread.sleep(1500)
            report("after", screen)
            val expected = arguments.getString("expect")?.split("|")?.map { it.trim() }?.filter { it.isNotEmpty() }
            val missing = expected?.let { expect(it, arguments.getString("expect_wait")?.toLong() ?: 5) }
            val name = "keys-${SystemClock.uptimeMillis()}.png"
            instrumentation.uiAutomation.takeScreenshot()?.let { shot ->
                FileOutputStream(File(instrumentation.targetContext.cacheDir, name)).use { shot.compress(Bitmap.CompressFormat.PNG, 100, it) }
                instrumentation.sendStatus(0, Bundle().apply { putString("fixture_screenshot", "cache/$name") })
            }
            Thread.sleep(hold * 1000)
            if (missing != null) assertTrue("not on screen: $missing", missing.isEmpty())
        }
    }

    /** Waits for each label in the app's accessibility tree; the ones never found. */
    private fun expect(labels: List<String>, seconds: Long): List<String> {
        val until = SystemClock.uptimeMillis() + seconds * 1000
        var counts: Map<String, Int>
        while (true) {
            counts = labels.associateWith { 0 }.toMutableMap().also { found ->
                instrumentation.uiAutomation.rootInActiveWindow?.let { root -> count(root, found) }
            }
            if (counts.values.all { it > 0 } || SystemClock.uptimeMillis() > until) break
            Thread.sleep(250)
        }
        instrumentation.sendStatus(
            0,
            Bundle().apply {
                putString("fixture_expect", labels.joinToString(", ") { "'$it' ${counts[it]}" })
                putString("fixture_expect_result", if (counts.values.all { it > 0 }) "PASS" else "FAIL")
            },
        )
        return labels.filter { counts[it] == 0 }
    }

    private fun count(node: AccessibilityNodeInfo, found: MutableMap<String, Int>) {
        val said = listOfNotNull(
            node.text,
            node.contentDescription,
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) node.hintText else null,
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) node.tooltipText else null,
        ).map { it.toString() }
        // Flutter may join a node's label, value and hint with new lines.
        val lines = said + said.flatMap { it.split('\n') }.map { it.trim() }
        for (label in found.keys) if (label in lines) found[label] = found.getValue(label) + 1
        for (index in 0 until node.childCount) node.getChild(index)?.let { count(it, found) }
    }

    private fun send(chord: KeyChord) {
        for ((action, code, meta) in chord.events()) {
            val now = SystemClock.uptimeMillis()
            val event = KeyEvent(
                now, now, action, code, 0, meta,
                KeyCharacterMap.VIRTUAL_KEYBOARD, 0, 0, InputDevice.SOURCE_KEYBOARD,
            )
            val sent = instrumentation.uiAutomation.injectInputEvent(event, true)
            instrumentation.sendStatus(
                0,
                Bundle().apply {
                    putString(
                        "fixture_key",
                        "${KeyEvent.keyCodeToString(code)} ${if (action == KeyEvent.ACTION_DOWN) "down" else "up"} " +
                            "meta=0x${Integer.toHexString(meta)} sent=$sent",
                    )
                },
            )
        }
    }

    private fun tap(x: Float, y: Float) {
        val now = SystemClock.uptimeMillis()
        for (action in listOf(MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP)) {
            val event = MotionEvent.obtain(now, now, action, x, y, 0).apply { source = InputDevice.SOURCE_TOUCHSCREEN }
            instrumentation.uiAutomation.injectInputEvent(event, true)
            event.recycle()
        }
    }

    private fun report(moment: String, screen: Screen) {
        var focus = ""
        screen.onActivity {
            focus = "${it.currentFocus?.javaClass?.simpleName ?: "nothing"}, touch mode ${it.window.decorView.isInTouchMode}"
        }
        instrumentation.sendStatus(0, Bundle().apply { putString("fixture_focus_$moment", focus) })
    }
}

/** The chords the fixture sends: parsed and ordered as a keyboard would. */
@RunWith(AndroidJUnit4::class)
class KeyChordTest {
    @Test
    fun ctrlFIsCtrlDownWithItsOwnMetaThenFThenReleasedInReverse() {
        val ctrl = KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON
        assertEquals(
            listOf(
                Triple(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_CTRL_LEFT, ctrl),
                Triple(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_F, ctrl),
                Triple(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_F, ctrl),
                Triple(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_CTRL_LEFT, 0),
            ),
            KeyChord.parse("ctrl+f").events(),
        )
    }

    @Test
    fun modifiersStackAndUnstack() {
        val ctrl = KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON
        val shift = KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON
        val events = KeyChord.parse("Ctrl+Shift+G").events()
        assertEquals(Triple(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_SHIFT_LEFT, ctrl or shift), events[1])
        assertEquals(Triple(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_G, ctrl or shift), events[2])
        assertEquals(Triple(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_SHIFT_LEFT, ctrl), events[4])
        assertEquals(Triple(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_CTRL_LEFT, 0), events[5])
    }

    @Test
    fun namedKeysAndRefusals() {
        assertEquals(listOf(KeyEvent.KEYCODE_ESCAPE), KeyChord.parse("escape").events().map { it.second }.distinct())
        assertEquals(KeyEvent.KEYCODE_ENTER, KeyChord.parse("shift+enter").key)
        assertEquals(KeyEvent.KEYCODE_F12, KeyChord.parse("f12").key)
        assertEquals(KeyEvent.KEYCODE_7, KeyChord.parse("ctrl+7").key)
        assertTrue(KeyChord.parse("ctrl+f").modifiers == listOf(KeyEvent.KEYCODE_CTRL_LEFT))
        for (bad in listOf("", "ctrl+", "hyper+f", "ctrl+ctrl+f", "ctrl+ff", "f13")) {
            try {
                KeyChord.parse(bad)
                fail("took '$bad'")
            } catch (expected: IllegalArgumentException) {
            }
        }
    }
}
