package com.ausha.receiver

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log

/**
 * Keeps playback alive with the screen off, and holds the locks that stop the
 * platform quietly ruining latency.
 */
class PlaybackService : Service() {

    private lateinit var audioManager: AudioManager
    private var wifiLock: WifiManager.WifiLock? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var focusRequest: AudioFocusRequest? = null
    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    private var target: Target? = null

    private data class Target(
        val host: String,
        val port: Int,
        val token: String,
        val name: String,
        val latency: AudioEngine.Latency,
    )

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        audioManager = getSystemService(Context.AUDIO_SERVICE) as AudioManager
        createChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopPlayback()
                stopSelf()
                return START_NOT_STICKY
            }
        }
        val host = intent?.getStringExtra(EXTRA_HOST) ?: return START_NOT_STICKY
        val next = Target(
            host = host,
            port = intent.getIntExtra(EXTRA_PORT, 6996),
            token = intent.getStringExtra(EXTRA_TOKEN).orEmpty(),
            name = intent.getStringExtra(EXTRA_NAME) ?: Build.MODEL,
            latency = AudioEngine.Latency.entries[
                intent.getIntExtra(EXTRA_LATENCY, AudioEngine.Latency.Balanced.ordinal)
                    .coerceIn(0, AudioEngine.Latency.entries.lastIndex)
            ],
        )
        // Pairing with a different sender while playing should switch to it,
        // and the engine will not restart itself while it is still running.
        if (next != target && Playback.engine.isRunning) {
            stopPlayback()
        }
        target = next

        startForegroundCompat()
        acquireLocks()
        watchNetwork()
        if (requestFocus()) startPlayback()
        return START_STICKY
    }

    override fun onDestroy() {
        stopPlayback()
        releaseLocks()
        unwatchNetwork()
        abandonFocus()
        super.onDestroy()
    }

    private fun startPlayback() {
        val target = target ?: return
        Playback.engine.start(
            target.host,
            target.port,
            target.token,
            target.name,
            target.latency,
        )
    }

    private fun stopPlayback() {
        Playback.engine.stop()
    }

    private fun startForegroundCompat() {
        val notification = buildNotification()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun buildNotification(): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, PlaybackService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(target?.host?.let { "Streaming from $it" } ?: "Streaming")
            .setSmallIcon(android.R.drawable.ic_lock_silent_mode_off)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, "Stop", stop).build())
            .setOngoing(true)
            .build()
    }

    private fun createChannel() {
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Playback",
            NotificationManager.IMPORTANCE_LOW,
        )
        channel.setShowBadge(false)
        (getSystemService(NotificationManager::class.java)).createNotificationChannel(channel)
    }

    /**
     * WiFi power save is the single biggest avoidable source of latency: an
     * idle radio parks between beacons and adds spikes that look exactly like
     * network jitter, which would make the jitter buffer grow to hide a
     * problem we caused ourselves.
     */
    private fun acquireLocks() {
        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            WifiManager.WIFI_MODE_FULL_LOW_LATENCY
        } else {
            @Suppress("DEPRECATION")
            WifiManager.WIFI_MODE_FULL_HIGH_PERF
        }
        wifiLock = wifi.createWifiLock(mode, "ausha:wifi").apply {
            setReferenceCounted(false)
            acquire()
        }

        val power = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "ausha:wake").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    private fun releaseLocks() {
        runCatching { wifiLock?.takeIf { it.isHeld }?.release() }
        runCatching { wakeLock?.takeIf { it.isHeld }?.release() }
        wifiLock = null
        wakeLock = null
    }

    /** Pause for calls and other apps rather than talking over them. */
    private fun requestFocus(): Boolean {
        val attributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
            .build()
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(attributes)
            .setOnAudioFocusChangeListener { change ->
                when (change) {
                    AudioManager.AUDIOFOCUS_LOSS -> {
                        stopPlayback()
                        stopSelf()
                    }
                    AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> stopPlayback()
                    AudioManager.AUDIOFOCUS_GAIN -> if (!Playback.engine.isRunning) startPlayback()
                }
            }
            .build()
        focusRequest = request
        return audioManager.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
    }

    private fun abandonFocus() {
        focusRequest?.let { audioManager.abandonAudioFocusRequest(it) }
        focusRequest = null
    }

    /**
     * Roaming between access points, or dropping to mobile data, changes the
     * local address and silently strands the UDP socket. Reconnecting is the
     * only way back; the sender's keepalive would otherwise take ten seconds
     * to notice on its side.
     */
    private fun watchNetwork() {
        val connectivity = getSystemService(ConnectivityManager::class.java)
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                if (!Playback.engine.isRunning && target != null) {
                    Log.i(TAG, "network back, reconnecting")
                    startPlayback()
                }
            }

            override fun onLost(network: Network) {
                Log.i(TAG, "network lost")
                stopPlayback()
            }
        }
        networkCallback = callback
        connectivity.registerNetworkCallback(
            NetworkRequest.Builder()
                .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .build(),
            callback,
        )
    }

    private fun unwatchNetwork() {
        networkCallback?.let {
            runCatching { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it) }
        }
        networkCallback = null
    }

    companion object {
        private const val TAG = "ausha"
        private const val CHANNEL_ID = "ausha.playback"
        private const val NOTIFICATION_ID = 1
        const val ACTION_STOP = "com.ausha.receiver.STOP"
        const val EXTRA_HOST = "host"
        const val EXTRA_PORT = "port"
        const val EXTRA_TOKEN = "token"
        const val EXTRA_NAME = "name"
        const val EXTRA_LATENCY = "latency"

        fun start(
            context: Context,
            host: String,
            port: Int,
            token: String,
            name: String,
            latency: AudioEngine.Latency,
        ) {
            val intent = Intent(context, PlaybackService::class.java)
                .putExtra(EXTRA_HOST, host)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_TOKEN, token)
                .putExtra(EXTRA_NAME, name)
                .putExtra(EXTRA_LATENCY, latency.ordinal)
            context.startForegroundService(intent)
        }

        fun stop(context: Context) {
            context.startService(
                Intent(context, PlaybackService::class.java).setAction(ACTION_STOP)
            )
        }
    }
}

/** One engine for the process, shared by the service and the UI. */
object Playback {
    @Volatile var state: AudioEngine.State = AudioEngine.State.Idle; private set

    val engine = AudioEngine { state = it }
}
