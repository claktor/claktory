package com.claktory.app

import android.os.Build
import androidx.test.platform.app.InstrumentationRegistry
import java.io.FileInputStream

/**
 * Grants the app under test [permission], as a person allowing it would:
 * through UiAutomation from Android 9, through the shell's `pm grant`
 * before (Android 7 and 8 have no UiAutomation grant).
 */
fun grant(permission: String) {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    val packageName = instrumentation.targetContext.packageName
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
        instrumentation.uiAutomation.grantRuntimePermission(packageName, permission)
    } else {
        instrumentation.uiAutomation.executeShellCommand("pm grant $packageName $permission").use { pipe ->
            FileInputStream(pipe.fileDescriptor).use { it.readBytes() }
        }
    }
}
