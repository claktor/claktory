package com.claktory.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class BackGestureExclusionTest {
    private fun boxes(rects: Any?, density: Float = 2.625f) = BackGestureExclusion.boxes(rects, density)?.map { it.toList() }

    @Test
    fun logicalRectsBecomeTheViewsPixelsCoveringAllTheyTouch() {
        // The mic: 48 logical px square, 26 from the right of a 411 wide screen, 2.625 px each.
        assertEquals(listOf(listOf(884, 4462, 1011, 4589)), boxes(listOf(listOf(337, 1700, 48, 48))))
        assertEquals(listOf(listOf(1, 2, 4, 6), listOf(0, 0, 0, 0)), boxes(listOf(listOf(0.5, 1.2, 1.5, 1.8), listOf(0, 0, 0, 0)), 2f))
        assertEquals(emptyList<List<Int>>(), boxes(emptyList<Any>()))
    }

    @Test
    fun anythingElseIsRefused() {
        assertNull(boxes(null))
        assertNull(boxes("rects"))
        assertNull(boxes(listOf(listOf(1, 2, 3))))
        assertNull(boxes(listOf(listOf(1, 2, 3, "4"))))
        assertNull(boxes(listOf(listOf(1, 2, -3, 4))))
        assertNull(boxes(listOf(listOf(1, 2, Double.NaN, 4))))
        assertNull(boxes(listOf(listOf(1, 2, Double.POSITIVE_INFINITY, 4))))
        assertNull(boxes(List(BackGestureExclusion.MAX + 1) { listOf(0, 0, 1, 1) }))
    }
}
