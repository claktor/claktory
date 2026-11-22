package com.claktory.app

import io.flutter.plugin.common.MethodChannel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WebAuthCoreTest {
    private class Answer : MethodChannel.Result {
        var answers = 0
        var value: Any? = null
        var code: String? = null
        var message: String? = null

        override fun success(result: Any?) {
            answers++
            value = result
        }

        override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
            answers++
            code = errorCode
            message = errorMessage
        }

        override fun notImplemented() {
            answers++
            code = "notImplemented"
        }
    }

    private val opened = mutableListOf<String>()
    private var opens = true
    private var backToApp = 0
    private val core = WebAuthCore(open = { opened += it; opens }, toApp = { backToApp++ })

    private val page = "https://dash.cloudflare.com/oauth2/auth?state=s1&redirect_uri=com.claktory.app:/callback"

    @Test
    fun theServicesRedirectToTheSchemeAskedForIsTheAnswer() {
        val answer = Answer()
        core.start(page, "com.claktory.app", answer)
        assertEquals(listOf(page), opened)
        core.paused()
        assertTrue(core.redirected("com.claktory.app:/callback?code=c1&state=s1"))
        assertEquals("com.claktory.app:/callback?code=c1&state=s1", answer.value)
        // The app coming back after it answers nothing more.
        core.resumed()
        assertEquals(1, answer.answers)
    }

    @Test
    fun anotherSchemeOrNoSignInWaitingIsntTaken() {
        assertFalse(core.redirected("com.claktory.app:/callback?code=forged"))
        val answer = Answer()
        core.start(page, "com.claktory.app", answer)
        assertFalse(core.redirected("claktory://add?code=x"))
        assertFalse(core.redirected("com.claktory.app.evil:/callback?code=x"))
        assertEquals(0, answer.answers)
        assertTrue(core.redirected("COM.CLAKTORY.APP:/callback?code=c1&state=s1"))
        assertEquals(1, answer.answers)
        assertFalse("taken once", core.redirected("com.claktory.app:/callback?code=again&state=s1"))
    }

    @Test
    fun aRedirectWithoutThePagesStateIsIgnoredAndTheSignInWaitsOn() {
        val answer = Answer()
        core.start(page, "com.claktory.app", answer)
        core.paused()
        assertFalse(core.redirected("com.claktory.app:/callback?code=forged"))
        assertFalse(core.redirected("com.claktory.app:/callback?code=forged&state=s2"))
        assertFalse(core.redirected("com.claktory.app:/callback?code=forged&state="))
        assertFalse(core.redirected("com.claktory.app:/callback?code=forged&state=s1x"))
        assertEquals("a forged redirect doesn't end it", 0, answer.answers)
        assertTrue(core.redirected("com.claktory.app:/callback?code=c1&state=s1"))
        assertEquals("com.claktory.app:/callback?code=c1&state=s1", answer.value)
    }

    @Test
    fun theStateMayComeBackEncodedOrInTheFragment() {
        val answer = Answer()
        core.start("https://auth.example/authorize?client_id=x&state=a%2Bb%3D", "com.claktory.app", answer)
        assertTrue(core.redirected("com.claktory.app:/cb#access_token=t&state=a%2Bb%3D"))
        val second = Answer()
        core.start("https://auth.example/authorize?state=a%2Bb%3D", "com.claktory.app", second)
        assertTrue(core.redirected("com.claktory.app:/cb?state=a+b%3D".replace("a+b", "a%2Bb")))
    }

    @Test
    fun aPageWithoutStateTakesTheFirstRedirectOnItsScheme() {
        val answer = Answer()
        core.start("https://example.com/sign-in", "com.claktory.app", answer)
        assertTrue(core.redirected("com.claktory.app:/callback?code=c1"))
        assertEquals(1, answer.answers)
    }

    @Test
    fun parametersAreReadFromTheQueryAndTheFragment() {
        assertEquals("s1", WebAuthCore.parameter("https://x/a?code=c&state=s1", "state"))
        assertEquals("s 1", WebAuthCore.parameter("https://x/a?state=s%201#frag", "state"))
        assertEquals("s2", WebAuthCore.parameter("app:/cb#state=s2", "state"))
        assertNull(WebAuthCore.parameter("https://x/a?mystate=s1", "state"))
        assertNull(WebAuthCore.parameter("https://x/a?state", "state"))
        assertNull(WebAuthCore.parameter("https://x/a", "state"))
    }

    @Test
    fun backInTheAppWithoutARedirectIsClosedFirst() {
        val answer = Answer()
        core.start(page, "com.claktory.app", answer)
        // Not yet shown: the app hasn't left the screen for it.
        core.resumed()
        assertEquals(0, answer.answers)
        core.paused()
        core.resumed()
        assertEquals(1, answer.answers)
        assertNull(answer.value)
        assertNull(answer.code)
    }

    @Test
    fun cancelAnswersNullAndBringsTheAppBackOverAShownPage() {
        val first = Answer()
        core.start(page, "com.claktory.app", first)
        core.cancel()
        assertEquals(1, first.answers)
        assertNull(first.value)
        assertEquals("nothing to close before it showed", 0, backToApp)
        val second = Answer()
        core.start(page, "com.claktory.app", second)
        core.paused()
        core.cancel()
        assertEquals(1, second.answers)
        assertEquals(1, backToApp)
        core.cancel()
        assertEquals(1, backToApp)
    }

    @Test
    fun abandoningAnswersNullWithoutBringingTheAppBack() {
        val answer = Answer()
        core.start(page, "com.claktory.app", answer)
        core.paused()
        core.abandon()
        assertEquals(1, answer.answers)
        assertNull(answer.value)
        assertEquals(0, backToApp)
        assertFalse(core.redirected("com.claktory.app:/callback?code=c1&state=s1"))
    }

    @Test
    fun aNewSignInClosesTheOneBefore() {
        val first = Answer()
        core.start(page, "com.claktory.app", first)
        val second = Answer()
        core.start(page, "com.claktory.app", second)
        assertEquals(1, first.answers)
        assertNull(first.value)
        assertEquals(0, second.answers)
    }

    @Test
    fun onlyWebAddressesOpen() {
        for (url in listOf(null, "javascript:alert(1)", "file:///data/data/x", "intent://x#Intent;end", "")) {
            val answer = Answer()
            core.start(url, "com.claktory.app", answer)
            assertEquals("arguments", answer.code)
            assertEquals("No address to open.", answer.message)
        }
        val answer = Answer()
        core.start(page, "", answer)
        assertEquals("arguments", answer.code)
        assertTrue(opened.isEmpty())
    }

    @Test
    fun noBrowserSaysSo() {
        opens = false
        val answer = Answer()
        core.start(page, "com.claktory.app", answer)
        assertEquals("failed", answer.code)
        assertEquals("Couldn't open the sign-in page.", answer.message)
        assertFalse(core.redirected("com.claktory.app:/callback?code=c1"))
    }

    @Test
    fun aSchemeIsReadOnlyFromAWellFormedAddress() {
        assertEquals("claktory", WebAuthCore.schemeOf("claktory://add?code=x"))
        assertEquals("com.claktory.app.dev", WebAuthCore.schemeOf("com.claktory.app.dev:/cb"))
        assertNull(WebAuthCore.schemeOf("no scheme here"))
        assertNull(WebAuthCore.schemeOf(":/cb"))
    }
}
