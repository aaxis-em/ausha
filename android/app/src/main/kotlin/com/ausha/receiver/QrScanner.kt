package com.ausha.receiver

import android.annotation.SuppressLint
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import java.util.concurrent.Executors

/** Camera preview that reports the first `ausha://` QR code it sees. */
@Composable
fun QrScanner(modifier: Modifier = Modifier, onPaired: (Pairing) -> Unit) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val executor = remember { Executors.newSingleThreadExecutor() }
    val scanner = remember { BarcodeScanning.getClient() }

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

                analysis.setAnalyzer(executor, analyze(scanner, onPaired))
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

@SuppressLint("UnsafeOptInUsageError")
private fun analyze(
    scanner: com.google.mlkit.vision.barcode.BarcodeScanner,
    onPaired: (Pairing) -> Unit,
) = ImageAnalysis.Analyzer { proxy ->
    val image = proxy.image
    if (image == null) {
        proxy.close()
        return@Analyzer
    }
    scanner.process(InputImage.fromMediaImage(image, proxy.imageInfo.rotationDegrees))
        .addOnSuccessListener { barcodes ->
            barcodes.firstOrNull { it.valueType == Barcode.TYPE_TEXT || it.rawValue != null }
                ?.rawValue
                ?.let { Pairing.parse(it) }
                ?.let(onPaired)
        }
        .addOnCompleteListener { proxy.close() }
}
