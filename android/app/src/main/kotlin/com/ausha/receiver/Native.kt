package com.ausha.receiver

/**
 * The Rust core. Everything above the audio device — the handshake, jitter
 * buffer, FEC, drift correction, the uplink encoder — lives behind these
 * calls.
 *
 * The handle is an opaque pointer. It must be passed back exactly as given.
 * [nativeFill] must only ever be called from the playback thread and
 * [nativeUplinkSend] only from the microphone thread; the core guards
 * everything else itself.
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
        uplink: Int,
    ): Long

    /** Returns how many samples were real audio rather than silence. */
    external fun nativeFill(handle: Long, output: FloatArray): Int

    /** Samples one uplink frame needs, or 0 if the sender granted no microphone. */
    external fun nativeUplinkFrame(handle: Long): Int

    /** Encodes and sends one captured frame. Returns 0 on success. */
    external fun nativeUplinkSend(handle: Long, input: FloatArray): Int

    external fun nativeStats(handle: Long): String

    external fun nativeIsRunning(handle: Long): Int

    external fun nativeDisconnect(handle: Long)
}
