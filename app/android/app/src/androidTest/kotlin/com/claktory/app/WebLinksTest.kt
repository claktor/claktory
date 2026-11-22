package com.claktory.app

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import io.flutter.plugin.common.MethodChannel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.util.Collections

/**
 * Links and web sign-in's way back, on a phone or emulator: which links
 * reach this build, that each reaches the app's one screen (never a
 * second), at launch or while it runs, and that a sign-in page's redirect
 * answers the waiting sign-in and brings the app back over the page. A
 * real service's sign-in in a real browser is checked by hand.
 */
@RunWith(AndroidJUnit4::class)
class WebLinksTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val heard: MutableList<String> = Collections.synchronizedList(mutableListOf())

    /** This build's link scheme and application id: Claktory Dev's in a dev build. */
    private val scheme = context.getString(R.string.link_scheme)
    private val id = context.packageName

    @After
    fun tearDown() {
        LinksChannel.heard = null
    }

    private fun waitFor(what: String, seconds: Int = 10, check: () -> Boolean) {
        val until = System.currentTimeMillis() + seconds * 1000L
        while (System.currentTimeMillis() < until) {
            if (check()) return
            Thread.sleep(100)
        }
        throw AssertionError("no $what within $seconds s")
    }

    private fun handlers(link: String) =
        context.packageManager.queryIntentActivities(Intent(Intent.ACTION_VIEW, Uri.parse(link)), PackageManager.MATCH_ALL)
            .filter { it.activityInfo.packageName == id }
            .map { it.activityInfo.name }

    private fun mainScreens(): List<Activity> {
        var screens = emptyList<Activity>()
        instrumentation.runOnMainSync {
            screens = Stage.values().filter { it != Stage.DESTROYED }.flatMap { ActivityLifecycleMonitorRegistry.getInstance().getActivitiesInStage(it) }
                .filterIsInstance<MainActivity>()
                .distinct()
        }
        return screens
    }

    private fun open(link: String) =
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(link)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))

    @Test
    fun onlyThisBuildsLinksReachIt() {
        assertEquals("claktory-dev", scheme)
        assertEquals(listOf(LinkActivity::class.java.name), handlers("$scheme://add?code=abc"))
        assertEquals(listOf(LinkActivity::class.java.name), handlers("$id:/callback?code=c1"))
        assertTrue("claktory:// is the real Claktory's", handlers("claktory://add?code=abc").isEmpty())
        assertTrue(handlers("com.claktory.app:/callback?code=c1").isEmpty())
    }

    @Test
    fun aLinkThatLaunchesTheAppWaitsForIt() {
        LinksChannel.heard = { heard += it }
        val link = "$scheme://add?code=launch"
        val launch = Intent(context, MainActivity::class.java).setAction(Intent.ACTION_VIEW).setData(Uri.parse(link))
        Screen.launch(launch).use {
            waitFor("the link") { heard.contains(link) }
        }
    }

    @Test
    fun aLinkWhileItRunsReachesTheSameScreen() {
        LinksChannel.heard = { heard += it }
        Screen.launch().use { scenario ->
            var first: Activity? = null
            scenario.onActivity { first = it }
            val kept = DeviceStores.credentials(context).read()
            val link = "$scheme://add?code=running"
            open(link)
            waitFor("the link") { heard.contains(link) }
            // A link only reaches the app, which asks the person first
            // (links_test.dart): nothing native connects or keeps anything.
            Thread.sleep(500)
            assertEquals(kept, DeviceStores.credentials(context).read())
            waitFor("the screen back") { mainScreens().size == 1 }
            assertTrue("the same screen, not a second", mainScreens().single() === first)
            assertFalse("taken once", heard.count { it == link } > 1)
        }
    }

    @Test
    fun aSignInsRedirectAnswersItAndBringsTheAppBack() {
        Screen.launch().use { scenario ->
            val answers: MutableList<Any?> = Collections.synchronizedList(mutableListOf())
            val failed: MutableList<String> = Collections.synchronizedList(mutableListOf())
            val answer = object : MethodChannel.Result {
                override fun success(result: Any?) {
                    answers += result
                }

                override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
                    failed += errorCode
                }

                override fun notImplemented() {
                    failed += "notImplemented"
                }
            }
            scenario.onActivity { WebAuth.current!!.core.start("https://example.invalid/sign-in", id, answer) }
            Thread.sleep(1_500)
            assumeTrue("a browser to show the page in", failed.isEmpty())
            val back = "$id:/callback?code=c1&state=s1"
            open(back)
            waitFor("the answer") { answers.isNotEmpty() }
            assertEquals(listOf<Any?>(back), answers.toList())
            waitFor("the app back over the page") {
                var resumed = false
                instrumentation.runOnMainSync {
                    resumed = ActivityLifecycleMonitorRegistry.getInstance().getActivitiesInStage(Stage.RESUMED).any { it is MainActivity }
                }
                resumed
            }
            assertEquals(1, mainScreens().size)
        }
    }
}
