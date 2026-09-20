package com.ausha.receiver

import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import com.google.zxing.BinaryBitmap
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.ReaderException
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import java.util.concurrent.Executors

/** Camera preview that reports the first `ausha://` QR code it sees. */
@Composable
fun QrScanner(modifier: Modifier = Modifier, onPaired: (Pairing) -> Unit) {
    val lifecycleOwner = LocalLifecycleOwner.current
    val executor = remember { Executors.newSingleThreadExecutor() }
    val reader = remember { QRCodeReader() }

    AndroidView(
        modifier = modifier,
        factory = { viewContext ->
            val previewView = PreviewView(viewContext)
            val providerFuture = ProcessCameraProvider.getInstance(viewContext)
            providerFuture.addListener({
                val provider = providerFuture.get()
                val preview = Preview.Builder().build().also {
                    it.setSurfaceProvider(previewView.surfaceProvider)
                }
                val analysis = ImageAnalysis.Builder()
                    .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                    .build()

                analysis.setAnalyzer(executor, analyze(reader, onPaired))
                runCatching {
                    provider.unbindAll()
                    provider.bindToLifecycle(
                        lifecycleOwner,
                        CameraSelector.DEFAULT_BACK_CAMERA,
                        preview,
                        analysis,
                    )
                }
            }, ContextCompat.getMainExecutor(viewContext))
            previewView
        },
    )
}

private fun analyze(reader: QRCodeReader, onPaired: (Pairing) -> Unit) =
    ImageAnalysis.Analyzer { proxy ->
        try {
            proxy.decodeQr(reader)
                ?.let { Pairing.parse(it) }
                ?.let(onPaired)
        } finally {
            proxy.close()
        }
    }

/** QR detection is rotation invariant, so the frame's rotation is ignored. */
private fun ImageProxy.decodeQr(reader: QRCodeReader): String? {
    val luma = planes[0]
    val bytes = ByteArray(luma.buffer.remaining())
    luma.buffer.get(bytes)
    return decodeQrCode(reader, bytes, luma.rowStride, width, height)
}

/**
 * Reads a QR code out of one frame's luma plane. A driver may pad each row, so
 * the source is given `rowStride` as its width and cropped back to the frame.
 */
internal fun decodeQrCode(
    reader: QRCodeReader,
    luma: ByteArray,
    rowStride: Int,
    width: Int,
    height: Int,
): String? {
    val source = PlanarYUVLuminanceSource(luma, rowStride, height, 0, 0, width, height, false)
    return try {
        reader.decode(BinaryBitmap(HybridBinarizer(source))).text
    } catch (_: ReaderException) {
        null
    } finally {
        reader.reset()
    }
}
