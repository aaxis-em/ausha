package com.ausha.receiver

/**
 * The Rust core. Everything above the audio device — the handshake, jitter
 * buffer, FEC, drift correction — lives behind these five calls.
 *
 * The handle is an opaque pointer. It must be passed back exactly as given,
 * and [nativeFill] must only ever be called from one thread.
 */
object Native {
    init {
        System.loadLibrary("ausha_mobile")
        nativeInitLogging()
    }

    external fun nativeInitLogging()

    /** Blocks through the handshake. Throws IOException if it fails. */
    external fun nativeConnect(
        host: String,
        port: Int,
        token: String,
        name: String,
        simulateLoss: Int,
        latency: Int,
    ): Long

    /** Returns how many samples were real audio rather than silence. */
    external fun nativeFill(handle: Long, output: FloatArray): Int

    external fun nativeStats(handle: Long): String

    external fun nativeIsRunning(handle: Long): Int

    external fun nativeDisconnect(handle: Long)
}
