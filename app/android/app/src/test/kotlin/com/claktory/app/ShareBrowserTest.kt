package com.claktory.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Only a shared website's own address opens (Fugo's W6 condition). */
class ShareBrowserTest {
    @Test
    fun aSharesOwnAddressOpensAsChecked() {
        assertEquals(
            "http://s-kmccoovm7ul5v6mtcqfs.localhost:21234/?code=abc#top",
            ShareBrowser.shareAddress("http://s-kmccoovm7ul5v6mtcqfs.localhost:21234/?code=abc#top"),
        )
        assertEquals("http://s-a1.localhost:3000/", ShareBrowser.shareAddress("http://s-a1.localhost:3000"))
        assertEquals("http://s-a1.localhost:3000/dash/d/x", ShareBrowser.shareAddress("http://s-a1.localhost:3000/dash/d/x"))
    }

    @Test
    fun anythingElseIsRefused() {
        for (url in listOf(
            null,
            "",
            "http://localhost:3000/",
            "http://127.0.0.1:3000/",
            "http://[::1]:3000/",
            "https://s-a1.localhost:3000/",
            "http://s-a1.localhost/",
            "http://s-a1.localhost:0/",
            "http://s-a1.localhost:70000/",
            "http://S-A1.localhost:3000/",
            "http://s-a1.localhost.evil.com:3000/",
            "http://evil.com/s-a1.localhost:3000/",
            "http://s-a1.localhost:3000@evil.com/",
            "http://user@s-a1.localhost:3000/",
            "http://x.s-a1.localhost:3000/",
            "http://s-.localhost:3000/",
            "http://s-a_1.localhost:3000/",
            "javascript:alert(1)",
            "intent://s-a1.localhost:3000/#Intent;scheme=http;end",
            "file:///data/data/com.claktory.app/x",
        )) {
            assertNull(url, ShareBrowser.shareAddress(url))
        }
    }
}
