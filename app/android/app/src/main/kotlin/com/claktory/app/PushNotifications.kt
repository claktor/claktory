package com.claktory.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle

/**
 * Shows the app's notifications on Android: pushes [ClaktoryMessagingService]
 * opened ([showPush]), the generic line for one it couldn't ([showGeneric]),
 * and what the app itself posts while it's in front ([showPosted]), each
 * in a channel by kind, so the person can tune them in Android's Settings.
 *
 * A tap opens the app's one screen. What it opens there is decided when it
 * arrives ([NotificationsChannel]): a push's only from its sealed text,
 * when that very push was shown (PushOpener.tapped); a posted one's only
 * from the target kept in this process under the id the tap carries. The
 * activity is exported, so nothing else an intent says is trusted.
 */
object PushNotifications {
    const val ACTION_OPEN = "com.claktory.app.action.NOTIFICATION"

    /** A push's sealed text, opened again on a tap. */
    const val EXTRA_SEALED = "sealed"

    /** The id a posted notification's target is kept under in this process. */
    const val EXTRA_POSTED = "posted"

    const val NEEDS_YOU = "needs_you"
    const val ALERTS = "alerts"
    const val REPLIES = "replies"
    const val TESTS = "tests"
    const val OTHER = "other"

    /** The channel for a push's kind. */
    fun channelFor(kind: String): String = when (kind) {
        "needs_you" -> NEEDS_YOU
        "alert" -> ALERTS
        "reply" -> REPLIES
        "test" -> TESTS
        else -> OTHER
    }

    /** Whether Android shows the app's notifications at all (the permission on 13+, the app's switch before). */
    fun allowed(context: Context): Boolean = manager(context).areNotificationsEnabled()

    /** The channels, made once (Android keeps what the person changes in them). */
    fun ensureChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = manager(context)
        for ((id, name) in listOf(
            NEEDS_YOU to R.string.channel_needs_you,
            ALERTS to R.string.channel_alerts,
            REPLIES to R.string.channel_replies,
            TESTS to R.string.channel_tests,
            OTHER to R.string.channel_other,
        )) {
            if (manager.getNotificationChannel(id) != null) continue
            manager.createNotificationChannel(
                NotificationChannel(id, context.getString(name), NotificationManager.IMPORTANCE_HIGH),
            )
        }
    }

    /**
     * A push that opened, from [claktory] (the id its key is kept under):
     * what it says, tagged as the app tags its own notice of the same
     * Notify ("claktory:id", notifications.dart), so one replaces the
     * other rather than doubling.
     */
    fun showPush(context: Context, claktory: String, notice: PushNotice, sealed: String, thread: String) {
        val tag = if (notice.id.isEmpty()) "push:$claktory:${notice.ctr}" else "$claktory:${notice.id}"
        val extras = Bundle().apply { putString(EXTRA_SEALED, sealed) }
        show(
            context,
            tag,
            channelFor(notice.kind),
            notice.title.ifEmpty { "Claktory" },
            notice.body,
            notice.claktory,
            thread,
            notice.urgent,
            notice.badge,
            extras,
            private = true,
        )
    }

    /** The generic line: whatever the push carried besides isn't read, and a tap only opens the app. */
    fun showGeneric(context: Context, thread: String) {
        show(
            context,
            "generic:${System.nanoTime()}",
            OTHER,
            "Claktory",
            context.getString(R.string.push_generic),
            null,
            thread,
            false,
            null,
            Bundle(),
            private = false,
        )
    }

    /** What the app posts (another Claktory than the one on screen, or any while the person isn't using the app). */
    fun showPosted(
        context: Context,
        id: String,
        title: String,
        body: String,
        thread: String,
        urgent: Boolean,
        kind: String,
        posted: String,
    ) {
        val extras = Bundle().apply { putString(EXTRA_POSTED, posted) }
        show(context, id, channelFor(kind), title.ifEmpty { "Claktory" }, body, null, thread, urgent, null, extras, private = true)
    }

    private fun show(
        context: Context,
        tag: String,
        channel: String,
        title: String,
        body: String,
        claktory: String?,
        thread: String,
        urgent: Boolean,
        badge: Int?,
        extras: Bundle,
        private: Boolean,
    ) {
        if (!allowed(context)) return
        ensureChannels(context)
        val builder = builder(context, channel, urgent)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setAutoCancel(true)
            .setContentIntent(tap(context, tag, extras))
        if (!claktory.isNullOrEmpty()) builder.setSubText(claktory)
        if (thread.isNotEmpty()) builder.setGroup(thread)
        if (badge != null) builder.setNumber(badge)
        if (urgent) builder.setCategory(Notification.CATEGORY_ALARM)
        if (private) {
            // On a lock screen set to hide what notifications say, only
            // the generic line shows (as the iPhone's previews).
            builder.setVisibility(Notification.VISIBILITY_PRIVATE).setPublicVersion(
                builder(context, channel, urgent)
                    .setSmallIcon(R.drawable.ic_notification)
                    .setContentTitle("Claktory")
                    .setContentText(context.getString(R.string.push_generic))
                    .build(),
            )
        } else {
            builder.setVisibility(Notification.VISIBILITY_PUBLIC)
        }
        try {
            manager(context).notify(tag, 0, builder.build())
        } catch (_: SecurityException) {
            // Taken away meanwhile (Android 13+): nothing to show it with.
        }
    }

    private fun builder(context: Context, channel: String, urgent: Boolean): Notification.Builder =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, channel)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(context).setPriority(if (urgent) Notification.PRIORITY_MAX else Notification.PRIORITY_HIGH)
                .setDefaults(Notification.DEFAULT_ALL)
        }

    /** The app's one screen, found in its own task, with what the tap is about. */
    private fun tap(context: Context, tag: String, extras: Bundle): PendingIntent {
        val intent = Intent(context, MainActivity::class.java)
            .setAction(ACTION_OPEN)
            .putExtras(extras)
            .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK)
        return PendingIntent.getActivity(
            context,
            tag.hashCode(),
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private fun manager(context: Context) = context.getSystemService(NotificationManager::class.java)
}

/** Whether the app's screen is in front: then a push doesn't show, since the app already has what it says. */
object Foreground {
    @Volatile
    var resumed = false
}
