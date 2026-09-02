package com.ausha.receiver

import org.json.JSONObject

/** What playback is doing, as the stats screen shows it. */
data class Stats(
    val received: Long = 0,
    val lost: Long = 0,
    val recovered: Long = 0,
    val concealed: Long = 0,
    val reordered: Long = 0,
    val late: Long = 0,
    val underruns: Long = 0,
    val jitterMs: Double = 0.0,
    val targetMs: Int = 0,
    val depthMs: Int = 0,
    val latencyMs: Double = 0.0,
    val ratio: Double = 1.0,
    val silenceFrames: Long = 0,
) {
    val lossPercent: Double
        get() = (received + lost).let { if (it == 0L) 0.0 else lost * 100.0 / it }

    companion object {
        fun parse(json: String): Stats {
            if (json.isEmpty()) return Stats()
            return runCatching {
                val root = JSONObject(json)
                val jitter = root.getJSONObject("jitter")
                Stats(
                    received = jitter.optLong("received"),
                    lost = jitter.optLong("lost"),
                    recovered = jitter.optLong("recovered"),
                    concealed = jitter.optLong("concealed"),
                    reordered = jitter.optLong("reordered"),
                    late = jitter.optLong("late"),
                    underruns = jitter.optLong("underruns"),
                    jitterMs = jitter.optDouble("jitter_ms", 0.0),
                    targetMs = jitter.optInt("target_ms"),
                    depthMs = jitter.optInt("depth_ms"),
                    latencyMs = root.optDouble("buffered_ms", 0.0),
                    ratio = root.optDouble("ratio", 1.0),
                    silenceFrames = root.optLong("silence_frames"),
                )
            }.getOrDefault(Stats())
        }
    }
}
