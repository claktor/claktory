package com.claktory.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class QrCodesTest {
    /** A pairing code's shape: the scheme, then a long base64url key and address. */
    private val pairing = "claktory://pair?k=" + "A1b2C3d4E5f6G7h8".repeat(8) + "&a=node.example:7420"

    /**
     * [modules] drawn as a camera would see them: [scale] pixels a module,
     * a quiet zone of four modules, dark on light, in rows of [stride]
     * bytes (wider than the picture, as a camera's often are).
     */
    private fun picture(size: Int, modules: ByteArray, scale: Int, stride: Int, invert: Boolean = false): Triple<ByteArray, Int, Int> {
        val side = (size + 8) * scale
        val luma = ByteArray(stride * side) { 0xE0.toByte() }
        for (y in 0 until side) {
            for (x in 0 until side) {
                val mx = x / scale - 4
                val my = y / scale - 4
                val dark = mx in 0 until size && my in 0 until size && modules[my * size + mx].toInt() == 1
                luma[y * stride + x] = if (dark != invert) 0x20 else 0xE0.toByte()
            }
        }
        return Triple(luma, side, side)
    }

    @Test
    fun aCodeIsItsModulesRowByRowWithNoQuietZone() {
        val code = QrCodes.modules(pairing)!!
        val size = code["size"] as Int
        val modules = code["modules"] as ByteArray
        assertEquals(size * size, modules.size)
        assertEquals("a version's side is 17 + 4 × version", 0, (size - 17) % 4)
        assertTrue(modules.all { it.toInt() == 0 || it.toInt() == 1 })
        // The finder patterns' dark corners: the code starts at its first module.
        assertEquals(1, modules[0].toInt())
        assertEquals(1, modules[size - 1].toInt())
        assertEquals(1, modules[(size - 1) * size].toInt())
        // Inside the top-left finder, the light ring.
        assertEquals(0, modules[size + 1].toInt())
    }

    @Test
    fun whatIsDrawnReadsBack() {
        val code = QrCodes.modules(pairing)!!
        val size = code["size"] as Int
        val (luma, width, height) = picture(size, code["modules"] as ByteArray, scale = 4, stride = (size + 8) * 4 + 32)
        assertEquals(pairing, QrCodes.read(luma, (size + 8) * 4 + 32, width, height))
    }

    @Test
    fun textBeyondAsciiReadsBackAsItWas() {
        val text = "Café ☕ Matt's Mac"
        val code = QrCodes.modules(text)!!
        val size = code["size"] as Int
        val (luma, width, height) = picture(size, code["modules"] as ByteArray, scale = 5, stride = (size + 8) * 5)
        assertEquals(text, QrCodes.read(luma, width, width, height))
    }

    @Test
    fun aFrameWithNoCodeReadsNothing() {
        val luma = ByteArray(320 * 240) { (it % 251).toByte() }
        assertNull(QrCodes.read(luma, 320, 320, 240))
    }

    @Test
    fun aCodeTooLongToBeAPairingCodeIsIgnored() {
        val long = "7".repeat(QrCodes.MAX_TEXT + 1)
        val code = QrCodes.modules(long)!!
        val size = code["size"] as Int
        val (luma, width, height) = picture(size, code["modules"] as ByteArray, scale = 2, stride = (size + 8) * 2)
        assertNull(QrCodes.read(luma, width, width, height))
    }

    @Test
    fun nothingOrTooMuchMakesNoCode() {
        assertNull(QrCodes.modules(""))
        assertNull(QrCodes.modules("x".repeat(8_000)))
    }

    @Test
    fun aPhoneHeldUprightFillsItsViewCropped() {
        // A 1920×1080 buffer from a sensor turned 90°, shown upright in a 1080×2000 view.
        val fill = QrCodes.fill(1080f, 2000f, 1920, 1080, sensorDegrees = 90, displayDegrees = 0)
        assertEquals(1f, fill.scaleX, 1e-4f)
        assertEquals(1920f / 2000f, fill.scaleY, 1e-4f)
        assertEquals(0f, fill.degrees, 0f)
        // 1080×1920 scaled to cover 1080×2000: by 2000/1920.
        assertEquals(2000f / 1920f, fill.scale, 1e-4f)
    }

    @Test
    fun aPhoneTurnedOnItsSideTurnsThePictureBack() {
        val fill = QrCodes.fill(2000f, 1080f, 1920, 1080, sensorDegrees = 90, displayDegrees = 90)
        assertEquals(-90f, fill.degrees, 0f)
        // Upright for the phone, 1080×1920; turned, 1920×1080, to cover 2000×1080.
        assertEquals(1080f / 2000f, fill.scaleX, 1e-4f)
        assertEquals(1920f / 1080f, fill.scaleY, 1e-4f)
        assertEquals(2000f / 1920f, fill.scale, 1e-4f)
    }

    @Test
    fun theLargestSizeWithinTheLimitOfTheSameShape() {
        val sizes = listOf(4000 to 3000, 1920 to 1080, 1440 to 1080, 1280 to 720, 640 to 480)
        assertEquals(1920 to 1080, QrCodes.largest(sizes, 1920, 1080))
        assertEquals(1280 to 720, QrCodes.largest(sizes, 1280, 720, like = 1920 to 1080))
        assertEquals(640 to 480, QrCodes.largest(listOf(640 to 480), 1280, 720, like = 1920 to 1080))
        assertNull(QrCodes.largest(listOf(4000 to 3000), 1280, 720))
        // The scanner's limit: a 16:9 camera reads 1280×720, a 4:3 one 1280×960, not 640×480.
        val camera = listOf(1920 to 1080, 1280 to 960, 1280 to 720, 640 to 480)
        assertEquals(1280 to 720, QrCodes.largest(camera, 1280, 960, like = 1920 to 1080))
        assertEquals(1280 to 960, QrCodes.largest(camera, 1280, 960, like = 1280 to 960))
    }

    @Test
    fun anInvertedCodeIsNotMistakenForAnother() {
        val code = QrCodes.modules(pairing)!!
        val size = code["size"] as Int
        val (luma, width, height) = picture(size, code["modules"] as ByteArray, scale = 4, stride = (size + 8) * 4, invert = true)
        val read = QrCodes.read(luma, width, width, height)
        assertTrue(read == null || read == pairing)
        assertArrayEquals(code["modules"] as ByteArray, QrCodes.modules(pairing)!!["modules"] as ByteArray)
    }

    @Test
    fun aSmallTiltedCodeReads() {
        // A Mac's pairing code (version 9, 53 modules) two pixels a module,
        // turned 10°, as a phone held well back sees it.
        val text = "factory://?t=" + "Ab3_".repeat(11) + "&name=Matt%27s%20MacBook%20Pro&e=" +
            "0123456789abcdef".repeat(4) + "&color=blue"
        val code = QrCodes.modules(text)!!
        val size = code["size"] as Int
        val modules = code["modules"] as ByteArray
        val (w, h) = 1280 to 720
        val module = 2.0
        val turn = Math.toRadians(10.0)
        val side = (size + 8) * module
        val luma = ByteArray(w * h) { i ->
            val dx = i % w - w / 2.0
            val dy = i / w - h / 2.0
            val u = Math.cos(turn) * dx + Math.sin(turn) * dy + side / 2
            val v = -Math.sin(turn) * dx + Math.cos(turn) * dy + side / 2
            val mx = Math.floor(u / module).toInt() - 4
            val my = Math.floor(v / module).toInt() - 4
            val paper = u >= 0 && v >= 0 && u < side && v < side
            val dark = paper && mx in 0 until size && my in 0 until size && modules[my * size + mx].toInt() == 1
            (if (!paper) 120 else if (dark) 70 else 170).toByte()
        }
        assertEquals(text, QrCodes.read(luma, w, w, h))
    }
}
