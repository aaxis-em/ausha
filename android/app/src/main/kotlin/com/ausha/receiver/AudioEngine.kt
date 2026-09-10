package com.ausha.receiver

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.os.Build
import android.os.Process
import android.util.Log
import java.io.IOException
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Pulls audio from the Rust core into an [AudioTrack] on a dedicated thread.
 *
 * The pull loop is deliberately the only caller of [Native.nativeFill]; the UI
 * reads [stats] instead, which the core guards separately.
 */
class AudioEngine(
    private val onState: (State) -> Unit,
) {
    enum class State { Idle, Connecting, Playing, Stopped, Failed }

    /**
     * Ordinals are the contract with the Rust side, so the order must not
     * change without changing `nativeConnect`.
     *
     * [Voice] is not offered as a preset: call mode selects it, because a
     * conversation needs a tighter buffer than any music setting.
     */
    enum class Latency {
        Low,
        Balanced,
        Stable,
        Voice;

        companion object {
            /** The presets a listener chooses between. */
            val presets = listOf(Low, Balanced, Stable)
        }
    }

    private val running = AtomicBoolean(false)
    private var thread: Thread? = null

    @Volatile private var handle: Long = 0
    @Volatile var stats: Stats = Stats(); private set
    @Volatile var failure: String? = null; private set

    fun start(
        host: String,
        port: Int,
        token: String,
        name: String,
        latency: Latency = Latency.Balanced,
        callMode: Boolean = false,
        simulateLoss: Int = 0,
    ) {
        if (running.getAndSet(true)) return
        failure = null
        onState(State.Connecting)
        thread = Thread(
            { run(host, port, token, name, latency, callMode, simulateLoss) },
            "ausha-audio",
        ).also { it.start() }
    }

    fun stop() {
        running.set(false)
        thread?.join(2000)
        thread = null
    }

    val isRunning: Boolean get() = running.get()

    private fun run(
        host: String,
        port: Int,
        token: String,
        name: String,
        latency: Latency,
        callMode: Boolean,
        simulateLoss: Int,
    ) {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        var track: AudioTrack? = null
        var microphone: Thread? = null
        try {
            handle = Native.nativeConnect(
                host,
                port,
                token,
                name,
                simulateLoss,
                latency.ordinal,
                if (callMode) 1 else 0,
            )
            if (handle == 0L) throw IOException("could not connect")
            if (callMode) microphone = startMicrophone(handle)

            track = buildTrack(callMode)
            // One 20 ms frame of stereo, matching the sender's packet cadence.
            val chunk = FloatArray(FRAME_SAMPLES * CHANNELS)
            track.play()
            onState(State.Playing)

            while (running.get() && Native.nativeIsRunning(handle) == 1) {
                Native.nativeFill(handle, chunk)
                // Blocking writes pace this loop against the audio clock.
                track.write(chunk, 0, chunk.size, AudioTrack.WRITE_BLOCKING)
                stats = Stats.parse(Native.nativeStats(handle))
            }
            onState(State.Stopped)
        } catch (e: Throwable) {
            Log.w(TAG, "playback failed", e)
            failure = e.message ?: e.javaClass.simpleName
            onState(State.Failed)
        } finally {
            running.set(false)
            // The microphone thread holds the same handle, so it has to be gone
            // before the handle is freed.
            runCatching { microphone?.join(2000) }
            runCatching { track?.stop() }
            runCatching { track?.release() }
            if (handle != 0L) {
                Native.nativeDisconnect(handle)
                handle = 0
            }
        }
    }

    /**
     * Captures and sends until playback stops.
     *
     * It lives here rather than in its own class because the native handle does:
     * whatever touches the handle has to be gone before [run] frees it, and one
     * owner makes that orderable.
     */
    private fun startMicrophone(handle: Long): Thread? {
        val frameSamples = Native.nativeUplinkFrame(handle)
        if (frameSamples <= 0) {
            Log.w(TAG, "call mode asked for but the sender granted no microphone")
            return null
        }
        return Thread({ captureLoop(handle, frameSamples) }, "ausha-mic").also { it.start() }
    }

    private fun captureLoop(handle: Long, frameSamples: Int) {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        val microphone = Microphone(frameSamples)
        try {
            if (!microphone.open()) return
            val frame = FloatArray(frameSamples)
            // The blocking read paces this loop against the capture clock, the
            // way the blocking write paces playback against the output clock.
            while (running.get()) {
                if (microphone.read(frame) != frameSamples) continue
                Native.nativeUplinkSend(handle, frame)
            }
        } catch (e: Throwable) {
            Log.w(TAG, "microphone failed", e)
        } finally {
            microphone.close()
        }
    }

    /**
     * Asks for the smallest buffer the device will give us and the low latency
     * path. Oversizing the buffer here would add delay the jitter buffer has
     * already been tuned to avoid.
     *
     * Call mode gives that up. The echo canceller only references playback on
     * the communication path, so the downlink has to be on it too, and that
     * path does not honour the low-latency mode.
     */
    private fun buildTrack(callMode: Boolean): AudioTrack {
        val minBytes = AudioTrack.getMinBufferSize(
            SAMPLE_RATE,
            AudioFormat.CHANNEL_OUT_STEREO,
            AudioFormat.ENCODING_PCM_FLOAT,
        )
        val builder = AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(
                        if (callMode) {
                            AudioAttributes.USAGE_VOICE_COMMUNICATION
                        } else {
                            AudioAttributes.USAGE_MEDIA
                        }
                    )
                    .setContentType(
                        if (callMode) {
                            AudioAttributes.CONTENT_TYPE_SPEECH
                        } else {
                            AudioAttributes.CONTENT_TYPE_MUSIC
                        }
                    )
                    .build()
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                    .setSampleRate(SAMPLE_RATE)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                    .build()
            )
            .setBufferSizeInBytes(maxOf(minBytes, FRAME_SAMPLES * CHANNELS * 4 * 2))
            .setTransferMode(AudioTrack.MODE_STREAM)

        if (!callMode && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            builder.setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
        }
        return builder.build()
    }

    companion object {
        private const val TAG = "ausha"
        const val SAMPLE_RATE = 48000
        const val CHANNELS = 2
        const val FRAME_MS = 20
        const val FRAME_SAMPLES = SAMPLE_RATE / 1000 * FRAME_MS

        /** Bluetooth adds latency the app cannot control; worth telling the user. */
        fun isBluetoothOutput(manager: AudioManager): Boolean =
            manager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).any {
                it.type == android.media.AudioDeviceInfo.TYPE_BLUETOOTH_A2DP ||
                    it.type == android.media.AudioDeviceInfo.TYPE_BLUETOOTH_SCO
            }
    }
}
