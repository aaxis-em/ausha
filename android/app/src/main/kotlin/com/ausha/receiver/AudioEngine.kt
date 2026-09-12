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
    @Volatile private var capture: Microphone? = null
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
            logRouting(track, callMode)

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
            runCatching { track?.stop() }
            runCatching { track?.release() }
            // The microphone thread sends into the same handle, so it has to be
            // gone before the handle is freed. Releasing the device first is
            // what makes that bounded: the thread is otherwise parked in a
            // blocking read that nothing else ends.
            capture?.wake()
            runCatching { microphone?.join(MICROPHONE_STOP_MS) }
            capture = null
            if (handle != 0L) {
                if (microphone?.isAlive == true) {
                    // Freeing the handle under a thread still sending into it
                    // is a use-after-free. Leaking one session is the lesser
                    // fault; the sender reaps it when the keepalive times out.
                    Log.e(TAG, "microphone thread did not stop; leaking the session")
                } else {
                    Native.nativeDisconnect(handle)
                }
                handle = 0
            }
        }
    }

    /**
     * Where the samples actually went, which is not always where they were
     * asked to go: the communication path picks its own output device, and may
     * downmix or resample on the way. Each of those is audible, and none of
     * them is visible from the stats screen.
     *
     * Device types are [android.media.AudioDeviceInfo] constants; 1 is the
     * earpiece and 2 the loudspeaker.
     */
    private fun logRouting(track: AudioTrack, callMode: Boolean) {
        Log.i(
            TAG,
            "playing: callMode=$callMode out=${track.routedDevice?.type} " +
                "rate=${track.sampleRate} channels=${track.channelCount}",
        )
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
        val microphone = Microphone(frameSamples)
        capture = microphone
        return Thread({ captureLoop(handle, microphone, frameSamples) }, "ausha-mic")
            .also { it.start() }
    }

    private fun captureLoop(handle: Long, microphone: Microphone, frameSamples: Int) {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
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

        /** Long enough for a released device to end a read, short of a hang. */
        private const val MICROPHONE_STOP_MS = 2000L

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
