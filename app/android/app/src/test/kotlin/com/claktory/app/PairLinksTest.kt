package com.claktory.app

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairLinksTest {
    @Test
    fun thePagesLinkWithItsCodeInTheFragment() {
        assertTrue(PairLinks.isPairLink("https://claktory.com/pair#t=abc&name=Studio&e=def&a=10.0.0.2%3A7420"))
        assertTrue(PairLinks.isPairLink("https://claktory.com/pair"))
        assertTrue("scheme and host aren't case-sensitive", PairLinks.isPairLink("HTTPS://Claktory.COM/pair#t=abc"))
    }

    @Test
    fun nothingElseOnTheWeb() {
        for (link in listOf(
            "http://claktory.com/pair#t=abc",
            "https://www.claktory.com/pair#t=abc",
            "https://claktory.com.evil.example/pair#t=abc",
            "https://evil.example/pair#t=abc",
            "https://claktory.com:8443/pair#t=abc",
            "https://user@claktory.com/pair#t=abc",
            "https://claktory.com/pair/#t=abc",
            "https://claktory.com/Pair#t=abc",
            "https://claktory.com/pairing#t=abc",
            "https://claktory.com/pair/x#t=abc",
            "https://claktory.com/%70air#t=abc",
            "https://claktory.com/android",
            "claktory://add?code=abc",
            "not a link",
            "",
        )) {
            assertFalse(link, PairLinks.isPairLink(link))
        }
    }
}
