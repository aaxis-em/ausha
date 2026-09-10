package com.ausha.receiver

import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.AudioEffect
import android.media.audiofx.AutomaticGainControl
import android.media.audiofx.NoiseSuppressor
import android.util.Log

/**
 * The phone's microphone, configured so that the platform cancels our own
 * playback out of it.
 *
 * Without that the far end of a call hears itself: we play their voice, the
 * speaker feeds it back into this microphone, and it goes straight back to
 * them. Android cancels it for free, but only on the communication audio
 * path — [MediaRecorder.AudioSource.VOICE_COMMUNICATION] here, matched by
 * `USAGE_VOICE_COMMUNICATION` on the [android.media.AudioTrack] playing the
 * downlink. The canceller references whatever the device is playing, and in
 * that configuration the downlink *is* what the device is playing, so the
 * reference signal is exactly the thing that needs removing.
 */
class Microphone(private val frameSamples: Int) {

    private var record: AudioRecord? = null
    private val effects = mutableListOf<AudioEffect>()

    /** Returns false rather than throwing when the device or the permission is not there. */
    fun open(): Boolean = runCatching {
        val minBytes = AudioRecord.getMinBufferSize(
            AudioEngine.SAMPLE_RATE,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_FLOAT,
        )
        if (minBytes <= 0) return@runCatching false

        val record = AudioRecord.Builder()
            .setAudioSource(MediaRecorder.AudioSource.VOICE_COMMUNICATION)
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                    .setSampleRate(AudioEngine.SAMPLE_RATE)
                    .setChannelMask(AudioFormat.CHANNEL_IN_MONO)
                    .build()
            )
            .setBufferSizeInBytes(maxOf(minBytes, frameSamples * 4 * 2))
            .build()

        if (record.state != AudioRecord.STATE_INITIALIZED) {
            record.release()
            return@runCatching false
        }
        attachEffects(record.audioSessionId)
        record.startRecording()
        this.record = record
        true
    }.getOrElse {
        Log.w(TAG, "microphone unavailable", it)
        false
    }

    /** Blocking, so the device paces whoever is reading. */
    fun read(into: FloatArray): Int =
        record?.read(into, 0, into.size, AudioRecord.READ_BLOCKING) ?: -1

    fun close() {
        effects.forEach { runCatching { it.release() } }
        effects.clear()
        record?.let {
            runCatching { it.stop() }
            runCatching { it.release() }
        }
        record = null
    }

    /**
     * `VOICE_COMMUNICATION` normally brings these with it. Enabling them
     * explicitly is what gets them on a device where it does not, and costs
     * nothing where it already has.
     */
    private fun attachEffects(sessionId: Int) {
        if (AcousticEchoCanceler.isAvailable()) {
            AcousticEchoCanceler.create(sessionId)?.let { enable(it, "echo canceller") }
        } else {
            Log.w(TAG, "no echo canceller on this device; the far end may hear itself")
        }
        if (NoiseSuppressor.isAvailable()) {
            NoiseSuppressor.create(sessionId)?.let { enable(it, "noise suppressor") }
        }
        if (AutomaticGainControl.isAvailable()) {
            AutomaticGainControl.create(sessionId)?.let { enable(it, "gain control") }
        }
    }

    private fun enable(effect: AudioEffect, what: String) {
        runCatching {
            effect.enabled = true
            effects += effect
            Log.i(TAG, "$what enabled")
        }.onFailure { Log.w(TAG, "could not enable $what", it) }
    }

    private companion object {
        const val TAG = "ausha"
    }
}
