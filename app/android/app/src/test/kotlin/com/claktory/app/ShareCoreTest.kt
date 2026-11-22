package com.claktory.app

import io.flutter.plugin.common.MethodChannel
import org.junit.Assert.assertEquals
import org.junit.Test

class ShareCoreTest {
    private class Answer : MethodChannel.Result {
        val values = mutableListOf<Any?>()

        override fun success(result: Any?) {
            values += result
        }

        override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
            values += "error $errorCode"
        }

        override fun notImplemented() {
            values += "notImplemented"
        }
    }

    private val shown = mutableListOf<String>()
    private var opens = true
    private val pending = mutableListOf<() -> Unit>()
    private val core = ShareCore(open = { shown += it; opens }, later = { _, run -> pending += run })

    private fun runLater() {
        val now = pending.toList()
        pending.clear()
        now.forEach { it() }
    }

    @Test
    fun pickingWhereToSendItAnswersTrue() {
        val answer = Answer()
        core.share("join with ABC-123", answer)
        assertEquals(listOf("join with ABC-123"), shown)
        core.paused()
        core.chosen()
        assertEquals(listOf<Any?>(true), answer.values)
        core.resumed()
        runLater()
        assertEquals("answered once", listOf<Any?>(true), answer.values)
    }

    @Test
    fun comingBackWithNothingChosenAnswersFalseAfterAMoment() {
        val answer = Answer()
        core.share("x", answer)
        core.paused()
        core.resumed()
        assertEquals(emptyList<Any?>(), answer.values)
        runLater()
        assertEquals(listOf<Any?>(false), answer.values)
    }

    @Test
    fun aLateWordOfTheChoiceStillCounts() {
        val answer = Answer()
        core.share("x", answer)
        core.paused()
        core.resumed()
        core.chosen()
        runLater()
        assertEquals(listOf<Any?>(true), answer.values)
    }

    @Test
    fun aResumeBeforeTheSheetShowedIsntAClose() {
        val answer = Answer()
        core.share("x", answer)
        core.resumed()
        runLater()
        assertEquals(emptyList<Any?>(), answer.values)
    }

    @Test
    fun theLastSharesLateCloseDoesntEndTheNext() {
        val first = Answer()
        core.share("one", first)
        core.paused()
        core.resumed()
        val second = Answer()
        core.share("two", second)
        assertEquals(listOf<Any?>(false), first.values)
        core.paused()
        runLater()
        assertEquals(emptyList<Any?>(), second.values)
        core.chosen()
        assertEquals(listOf<Any?>(true), second.values)
    }

    @Test
    fun nothingToShareOrNoSheetAnswersFalse() {
        val empty = Answer()
        core.share("", empty)
        core.share(null, empty)
        assertEquals(listOf<Any?>(false, false), empty.values)
        opens = false
        val none = Answer()
        core.share("x", none)
        assertEquals(listOf<Any?>(false), none.values)
        core.chosen()
        assertEquals(listOf<Any?>(false), none.values)
    }
}
