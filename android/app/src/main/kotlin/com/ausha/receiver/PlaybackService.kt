package com.ausha.receiver

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.support.v4.media.session.PlaybackStateCompat
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import androidx.media.session.MediaButtonReceiver
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/**
 * Keeps playback alive with the screen off, holds the locks that stop the
 * platform quietly ruining latency, and owns the MediaSession that headset
 * buttons and the lock screen talk to.
 */
class PlaybackService : Service(), Transport.Controls {

    private lateinit var audioManager: AudioManager
    private lateinit var transport: Transport
    private val scope = CoroutineScope(Dispatchers.Main.immediate)
    private var wifiLock: WifiManager.WifiLock? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var focusRequest: AudioFocusRequest? = null
    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    private var target: Target? = null

    /** The listener's own call volume, held while call mode overrides it. */
    private var savedCallVolume: Int? = null

    /**
     * Set while the user paused deliberately, so that regaining audio focus or
     * a network coming back does not restart a stream they stopped on purpose.
     */
    private var paused = false

    private data class Target(
        val host: String,
        val port: Int,
        val token: String,
        val name: String,
        val latency: AudioEngine.Latency,
        val callMode: Boolean,
    )

    /** Unplugging headphones should pause rather than play out loud. */
    private val becomingNoisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) = onPause()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        audioManager = getSystemService(Context.AUDIO_SERVICE) as AudioManager
        transport = Transport(this, this)
        createChannel()
        ContextCompat.registerReceiver(
            this,
            becomingNoisy,
            IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        scope.launch { Playback.state.collect(::onEngineState) }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            Intent.ACTION_MEDIA_BUTTON -> return onMediaButton(intent)
            ACTION_STOP -> {
                onStop()
                return START_NOT_STICKY
            }
        }
        val host = intent?.getStringExtra(EXTRA_HOST) ?: return START_NOT_STICKY
        val callMode = intent.getBooleanExtra(EXTRA_CALL_MODE, false) && canRecord()
        val next = Target(
            host = host,
            port = intent.getIntExtra(EXTRA_PORT, 6996),
            token = intent.getStringExtra(EXTRA_TOKEN).orEmpty(),
            name = intent.getStringExtra(EXTRA_NAME) ?: Build.MODEL,
            // Call mode brings its own buffer: a conversation needs a tighter
            // one than any listening preset, so it overrides the choice.
            latency = when {
                callMode -> AudioEngine.Latency.Voice
                else -> AudioEngine.Latency.presets[
                    intent.getIntExtra(EXTRA_LATENCY, AudioEngine.Latency.Balanced.ordinal)
                        .coerceIn(0, AudioEngine.Latency.presets.lastIndex)
                ]
            },
            callMode = callMode,
        )
        // Pairing with a different sender while playing should switch to it,
        // and the engine will not restart itself while it is still running.
        if (next != target && Playback.engine.isRunning) {
            stopPlayback()
        }
        target = next
        transport.describe(next.host)

        startForegroundCompat()
        // Playing first: registering the callback reports the current default
        // network straight away, and that arrives as a reconnect if the engine
        // is not already running.
        onPlay()
        watchNetwork()
        return START_STICKY
    }

    /**
     * The system starts the service to deliver a media button, so it has to
     * reach the foreground even when there is nothing to resume.
     */
    private fun onMediaButton(intent: Intent): Int {
        startForegroundCompat()
        transport.handleMediaButton(intent)
        if (target == null) stopSelf()
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        stopPlayback()
        releaseLocks()
        unwatchNetwork()
        abandonFocus()
        unregisterReceiver(becomingNoisy)
        transport.release()
        scope.cancel()
        super.onDestroy()
    }

    override fun onPlay() {
        paused = false
        acquireLocks()
        if (requestFocus()) startPlayback()
    }

    override fun onPause() {
        paused = true
        stopPlayback()
        abandonFocus()
        releaseLocks()
    }

    override fun onStop() {
        paused = false
        stopPlayback()
        stopSelf()
    }

    private fun onEngineState(state: AudioEngine.State) {
        transport.publish(state, Playback.engine.failure)
        if (target != null) {
            getSystemService(NotificationManager::class.java)
                .notify(NOTIFICATION_ID, buildNotification(state))
        }
    }

    private fun startPlayback() {
        val target = target ?: return
        // The echo canceller only works on the communication path, and the
        // whole path — capture, playback and this mode — has to be on it
        // together or it has nothing to cancel against.
        val mode = when {
            target.callMode -> AudioManager.MODE_IN_COMMUNICATION
            else -> AudioManager.MODE_NORMAL
        }
        audioManager.mode = mode
        // Setting the mode fails quietly when the platform refuses it, and a
        // refusal means no echo cancellation, so say so rather than leaving it
        // to be discovered by the person on the other end of the call.
        if (audioManager.mode != mode) {
            Log.w(TAG, "audio mode stayed ${audioManager.mode}, wanted $mode; no echo cancellation")
        }
        if (target.callMode) {
            routeCallAudio()
            raiseCallVolume()
        }
        Playback.engine.start(
            target.host,
            target.port,
            target.token,
            target.name,
            target.latency,
            target.callMode,
        )
    }

    private fun stopPlayback() {
        Playback.engine.stop()
        restoreCallVolume()
        clearCallRouting()
        audioManager.mode = AudioManager.MODE_NORMAL
    }

    /**
     * The voice call stream carries its own volume, on a scale calibrated for
     * an earpiece against an ear. On the loudspeaker, where call mode puts it,
     * whatever the system happened to be holding is usually close to inaudible.
     *
     * The old level is put back on the way out, so listening to a stream does
     * not quietly redefine how loud the listener's real phone calls are.
     */
    private fun raiseCallVolume() {
        if (savedCallVolume != null) return
        runCatching {
            val loudest = audioManager.getStreamMaxVolume(AudioManager.STREAM_VOICE_CALL)
            val current = audioManager.getStreamVolume(AudioManager.STREAM_VOICE_CALL)
            if (current >= loudest) return@runCatching
            savedCallVolume = current
            audioManager.setStreamVolume(AudioManager.STREAM_VOICE_CALL, loudest, 0)
        }
    }

    private fun restoreCallVolume() {
        val saved = savedCallVolume ?: return
        savedCallVolume = null
        runCatching {
            val loudest = audioManager.getStreamMaxVolume(AudioManager.STREAM_VOICE_CALL)
            // Only while it is still where this left it: a listener who reached
            // for the volume keys meant the level they chose to survive.
            if (audioManager.getStreamVolume(AudioManager.STREAM_VOICE_CALL) == loudest) {
                audioManager.setStreamVolume(AudioManager.STREAM_VOICE_CALL, saved, 0)
            }
        }
    }

    /**
     * Puts call audio on the loudspeaker.
     *
     * `MODE_IN_COMMUNICATION` routes playback to the earpiece, which is right
     * for a phone held against a head and wrong for one lying on a desk being
     * a speaker: the stream plays, inaudibly, out of the pinhole at the top.
     * Nothing moves it but an explicit choice. A headset still wins, because
     * plugging one in is a choice too.
     */
    private fun routeCallAudio() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val devices = audioManager.availableCommunicationDevices
            val wanted = devices.firstOrNull { it.type in HEADSETS }
                ?: devices.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
            wanted?.let { runCatching { audioManager.setCommunicationDevice(it) } }
            return
        }
        @Suppress("DEPRECATION")
        audioManager.isSpeakerphoneOn = !hasHeadset()
    }

    private fun clearCallRouting() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            runCatching { audioManager.clearCommunicationDevice() }
            return
        }
        @Suppress("DEPRECATION")
        audioManager.isSpeakerphoneOn = false
    }

    private fun hasHeadset(): Boolean =
        audioManager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).any { it.type in HEADSETS }

    /**
     * From API 30 a microphone foreground service needs the permission held
     * before `startForeground`, not merely declared, or it throws.
     */
    private fun canRecord(): Boolean =
        checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    private fun startForegroundCompat() {
        val notification = buildNotification(Playback.state.value)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            var types = ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
            if (target?.callMode == true) {
                types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
            }
            startForeground(NOTIFICATION_ID, notification, types)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun buildNotification(state: AudioEngine.State): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = MediaButtonReceiver.buildMediaButtonPendingIntent(
            this,
            PlaybackStateCompat.ACTION_STOP,
        )
        val live = state == AudioEngine.State.Playing || state == AudioEngine.State.Connecting
        val toggle = NotificationCompat.Action(
            if (live) R.drawable.ic_pause else R.drawable.ic_play,
            if (live) "Pause" else "Play",
            MediaButtonReceiver.buildMediaButtonPendingIntent(
                this,
                PlaybackStateCompat.ACTION_PLAY_PAUSE,
            ),
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(describe(state))
            .setSmallIcon(R.drawable.ic_ausha_mark)
            .setContentIntent(open)
            .addAction(toggle)
            .addAction(NotificationCompat.Action(R.drawable.ic_stop, "Stop", stop))
            .setStyle(
                androidx.media.app.NotificationCompat.MediaStyle()
                    .setMediaSession(transport.token)
                    .setShowActionsInCompactView(0, 1)
                    .setShowCancelButton(true)
                    .setCancelButtonIntent(stop)
            )
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(live)
            .build()
    }

    private fun describe(state: AudioEngine.State): String {
        val host = target?.host ?: return "Not connected"
        return when (state) {
            AudioEngine.State.Playing -> when (target?.callMode) {
                true -> "Call mode with $host"
                else -> "Streaming from $host"
            }
            AudioEngine.State.Connecting -> "Connecting to $host…"
            AudioEngine.State.Failed -> Playback.engine.failure ?: "Playback failed"
            AudioEngine.State.Idle, AudioEngine.State.Stopped ->
                if (paused) "Paused" else "Not streaming"
        }
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
        if (wifiLock != null) return
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
                    AudioManager.AUDIOFOCUS_LOSS -> onStop()
                    AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> stopPlayback()
                    AudioManager.AUDIOFOCUS_GAIN ->
                        if (!paused && !Playback.engine.isRunning) startPlayback()
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
     *
     * This watches the default network rather than every network with
     * internet: a phone holding both WiFi and mobile data raises `onLost` for
     * whichever it drops, and stopping playback for a network we were not
     * using cut the stream on a device that had not lost anything.
     */
    private fun watchNetwork() {
        if (networkCallback != null) return
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                if (!paused && !Playback.engine.isRunning && target != null) {
                    Log.i(TAG, "default network back, reconnecting")
                    startPlayback()
                }
            }

            override fun onLost(network: Network) {
                Log.i(TAG, "default network lost")
                stopPlayback()
            }
        }
        networkCallback = callback
        getSystemService(ConnectivityManager::class.java)
            .registerDefaultNetworkCallback(callback)
    }

    private fun unwatchNetwork() {
        networkCallback?.let {
            runCatching { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it) }
        }
        networkCallback = null
    }

    companion object {
        private const val TAG = "ausha"

        /** Outputs a listener chose deliberately, which outrank the loudspeaker. */
        private val HEADSETS = setOf(
            AudioDeviceInfo.TYPE_WIRED_HEADSET,
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
            AudioDeviceInfo.TYPE_USB_HEADSET,
            AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
            AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
        )

        private const val CHANNEL_ID = "ausha.playback"
        private const val NOTIFICATION_ID = 1
        const val ACTION_STOP = "com.ausha.receiver.STOP"
        const val EXTRA_HOST = "host"
        const val EXTRA_PORT = "port"
        const val EXTRA_TOKEN = "token"
        const val EXTRA_NAME = "name"
        const val EXTRA_LATENCY = "latency"
        const val EXTRA_CALL_MODE = "call_mode"

        fun start(
            context: Context,
            host: String,
            port: Int,
            token: String,
            name: String,
            latency: AudioEngine.Latency,
            callMode: Boolean = false,
        ) {
            val intent = Intent(context, PlaybackService::class.java)
                .putExtra(EXTRA_HOST, host)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_TOKEN, token)
                .putExtra(EXTRA_NAME, name)
                .putExtra(EXTRA_LATENCY, latency.ordinal)
                .putExtra(EXTRA_CALL_MODE, callMode)
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
    private val current = MutableStateFlow(AudioEngine.State.Idle)

    val state: StateFlow<AudioEngine.State> = current

    val engine = AudioEngine { current.value = it }
}
