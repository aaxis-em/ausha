package com.ausha.receiver

import com.google.zxing.BarcodeFormat
import com.google.zxing.qrcode.QRCodeReader
import com.google.zxing.qrcode.QRCodeWriter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Covers the decoder against the shape a camera actually delivers: a luma plane
 * whose rows are wider than the frame.
 */
class QrScannerTest {

    @Test
    fun `reads a pairing link out of a camera frame`() {
        val link = "ausha://192.168.1.42:6996?token=abc123&name=laptop"
        val frame = frameContaining(link, rowPadding = 0)

        assertEquals(link, decodeQrCode(QRCodeReader(), frame.luma, frame.rowStride, SIZE, SIZE))
    }

    @Test
    fun `reads it when the driver pads every row`() {
        val link = "ausha://10.0.0.5:6996?token=deadbeef"
        val frame = frameContaining(link, rowPadding = 48)

        assertEquals(link, decodeQrCode(QRCodeReader(), frame.luma, frame.rowStride, SIZE, SIZE))
    }

    @Test
    fun `returns null on a frame with no code in it`() {
        val blank = Frame(ByteArray(SIZE * SIZE) { -1 }, SIZE)

        assertNull(decodeQrCode(QRCodeReader(), blank.luma, blank.rowStride, SIZE, SIZE))
    }

    @Test
    fun `the same reader keeps working across frames`() {
        val reader = QRCodeReader()
        val blank = Frame(ByteArray(SIZE * SIZE) { -1 }, SIZE)
        val link = "ausha://172.16.0.9:6996?token=cafe"

        assertNull(decodeQrCode(reader, blank.luma, blank.rowStride, SIZE, SIZE))
        val frame = frameContaining(link, rowPadding = 16)
        assertEquals(link, decodeQrCode(reader, frame.luma, frame.rowStride, SIZE, SIZE))
    }

    private data class Frame(val luma: ByteArray, val rowStride: Int)

    /** A greyscale frame holding `text` as a QR code, dark on light. */
    private fun frameContaining(text: String, rowPadding: Int): Frame {
        val matrix = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, SIZE, SIZE)
        val rowStride = SIZE + rowPadding
        val luma = ByteArray(rowStride * SIZE) { -1 }

        for (y in 0 until SIZE) {
            for (x in 0 until SIZE) {
                if (matrix[x, y]) luma[y * rowStride + x] = 0
            }
        }
        return Frame(luma, rowStride)
    }

    private companion object {
        const val SIZE = 240
    }
}
