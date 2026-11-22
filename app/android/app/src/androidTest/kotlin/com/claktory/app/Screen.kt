package com.claktory.app

import android.content.Intent
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage

/**
 * The app's screen for a test, started and finished with the
 * instrumentation itself. ActivityScenario in the androidx.test the
 * integration_test plugin pins (1.3) registers a receiver Android 14+
 * refuses, and a newer one can't come in beside that pin.
 */
class Screen private constructor(val activity: MainActivity) : AutoCloseable {
    fun onActivity(body: (MainActivity) -> Unit) {
        InstrumentationRegistry.getInstrumentation().runOnMainSync { body(activity) }
    }

    override fun close() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        instrumentation.runOnMainSync { activity.finish() }
        val until = System.currentTimeMillis() + 10_000
        while (System.currentTimeMillis() < until) {
            var gone = false
            instrumentation.runOnMainSync {
                gone = ActivityLifecycleMonitorRegistry.getInstance().getLifecycleStageOf(activity) == Stage.DESTROYED
            }
            if (gone) return
            Thread.sleep(50)
        }
    }

    companion object {
        fun launch(intent: Intent? = null): Screen {
            val instrumentation = InstrumentationRegistry.getInstrumentation()
            val context = instrumentation.targetContext
            val start = (intent ?: Intent(context, MainActivity::class.java))
                .setClass(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            return Screen(instrumentation.startActivitySync(start) as MainActivity)
        }
    }
}
