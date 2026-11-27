package com.claktory.app

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * How the microphone's and camera's permissions read for the permission
 * sheet (area 5a): Before while Android can still ask, Declined only once
 * it won't.
 */
class PermissionStateTest {
    @After
    fun tearDown() = Refusals.forget()

    @Test
    fun allowedIsGrantedWhateverCameBefore() {
        assertEquals("granted", PermissionState.of(granted = true, refusedForGood = true, androidWouldExplain = false))
        assertEquals("granted", PermissionState.of(granted = true, refusedForGood = false, androidWouldExplain = true))
    }

    @Test
    fun neverAskedOrNotKnownIsUndetermined() {
        // Never asked, or refused for good in an earlier run: Android reads
        // the same before asking, and nothing is kept on the device.
        assertEquals("undetermined", PermissionState.of(granted = false, refusedForGood = false, androidWouldExplain = false))
    }

    @Test
    fun refusedOnceIsStillUndeterminedSoTheSheetAsksAgain() {
        // Android's first no: it asks a second time, and says so.
        assertEquals("undetermined", PermissionState.of(granted = false, refusedForGood = true, androidWouldExplain = true))
        assertEquals("undetermined", PermissionState.of(granted = false, refusedForGood = false, androidWouldExplain = true))
    }

    @Test
    fun refusedWhereAndroidWontAskAgainIsDenied() {
        assertEquals("denied", PermissionState.of(granted = false, refusedForGood = true, androidWouldExplain = false))
    }

    @Test
    fun aSecondNoIsForGood() {
        assertTrue(PermissionState.refusedForGood(explainedBefore = true, explainsAfter = false, elapsedMs = 4_000))
    }

    @Test
    fun aFirstNoIsNot() {
        assertFalse(PermissionState.refusedForGood(explainedBefore = false, explainsAfter = true, elapsedMs = 4_000))
    }

    @Test
    fun aQuestionSetAsideIsNot() {
        // Back, or a tap beside it, after the person saw it: Android asks again.
        assertFalse(PermissionState.refusedForGood(explainedBefore = false, explainsAfter = false, elapsedMs = 2_500))
        // Set aside after a first no: still explaining.
        assertFalse(PermissionState.refusedForGood(explainedBefore = true, explainsAfter = true, elapsedMs = 2_500))
    }

    @Test
    fun aNoWithNoQuestionShownIsForGood() {
        // Refused for good in an earlier run, or fixed by the phone's
        // administrator: Android answers at once, and slow phones err here.
        assertTrue(PermissionState.refusedForGood(explainedBefore = false, explainsAfter = false, elapsedMs = 60))
        assertTrue(PermissionState.refusedForGood(explainedBefore = false, explainsAfter = false, elapsedMs = PermissionState.UNSEEN_MS - 1))
    }

    @Test
    fun refusalsAreKeptPerPermissionUntilAllowed() {
        val mic = "android.permission.RECORD_AUDIO"
        val camera = "android.permission.CAMERA"
        Refusals.asking(mic, explains = true, at = 1_000)
        Refusals.answered(mic, allowed = false, explains = false, at = 3_000)
        assertTrue(Refusals.refusedForGood(mic))
        assertFalse(Refusals.refusedForGood(camera))
        Refusals.asking(mic, explains = false, at = 5_000)
        Refusals.answered(mic, allowed = true, explains = false, at = 7_000)
        assertFalse(Refusals.refusedForGood(mic))
    }

    @Test
    fun aQuestionSetAsideForgetsNothingAndAddsNothing() {
        val mic = "android.permission.RECORD_AUDIO"
        Refusals.asking(mic, explains = false, at = 1_000)
        Refusals.answered(mic, allowed = false, explains = false, at = 4_000)
        assertFalse(Refusals.refusedForGood(mic))
    }

    @Test
    fun refusedOnceOnlyWhileAndroidWillStillAsk() {
        assertTrue(PermissionState.refusedOnce(granted = false, androidWouldExplain = true))
        assertFalse(PermissionState.refusedOnce(granted = false, androidWouldExplain = false))
        assertFalse(PermissionState.refusedOnce(granted = true, androidWouldExplain = true))
    }
}
