package com.claktory.app

import android.app.Instrumentation
import android.os.ParcelFileDescriptor

/**
 * A shell command run as the shell user, its output read to the end
 * before the pipe is closed. Closing it unread breaks the system's writer
 * (EPIPE), which on Android 7 kills the whole instrumentation run.
 */
internal fun Instrumentation.shell(command: String): String =
    ParcelFileDescriptor.AutoCloseInputStream(uiAutomation.executeShellCommand(command))
        .bufferedReader().use { it.readText() }
