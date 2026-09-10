package com.ausha.receiver

import android.content.Context
import android.content.Intent
import android.os.SystemClock
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import androidx.media.session.MediaButtonReceiver

/**
 * The MediaSession: what headset buttons, the lock screen and the system media
 * panel talk to.
 *
 * A live stream has no seek, no skip and no duration, so the session offers
 * only play, pause and stop. Pause means leaving the sender, since there is no
 * backlog to resume from — reconnecting is what "play" does.
 */
class Transport(
    context: Context,
    private val controls: Controls,
) {
    interface Controls {
        fun onPlay()
        fun onPause()
        fun onStop()
    }

    private val session = MediaSessionCompat(context, TAG).apply {
        setCallback(object : MediaSessionCompat.Callback() {
            override fun onPlay() = controls.onPlay()
            override fun onPause() = controls.onPause()
            override fun onStop() = controls.onStop()
        })
    }

    val token: MediaSessionCompat.Token get() = session.sessionToken

    /** Routes a media button intent the system delivered to the service. */
    fun handleMediaButton(intent: Intent) {
        MediaButtonReceiver.handleIntent(session, intent)
    }

    fun describe(sender: String) {
        session.setMetadata(
            MediaMetadataCompat.Builder()
                .putString(MediaMetadataCompat.METADATA_KEY_TITLE, "Ausha")
                .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, sender)
                .build()
        )
    }

    /**
     * The session has to be active for headset buttons to reach it, and
     * inactive once playback ends so the buttons go back to whichever app the
     * user was listening to before.
     */
    fun publish(state: AudioEngine.State, failure: String?) {
        val playbackState = when (state) {
            AudioEngine.State.Playing -> PlaybackStateCompat.STATE_PLAYING
            AudioEngine.State.Connecting -> PlaybackStateCompat.STATE_BUFFERING
            AudioEngine.State.Stopped -> PlaybackStateCompat.STATE_PAUSED
            AudioEngine.State.Idle -> PlaybackStateCompat.STATE_NONE
            AudioEngine.State.Failed -> PlaybackStateCompat.STATE_ERROR
        }
        session.isActive = playbackState != PlaybackStateCompat.STATE_NONE

        val builder = PlaybackStateCompat.Builder()
            .setActions(
                PlaybackStateCompat.ACTION_PLAY or
                    PlaybackStateCompat.ACTION_PAUSE or
                    PlaybackStateCompat.ACTION_PLAY_PAUSE or
                    PlaybackStateCompat.ACTION_STOP
            )
            // A live stream has no position to report, and claiming one would
            // make the system draw a progress bar that never moves.
            .setState(
                playbackState,
                PlaybackStateCompat.PLAYBACK_POSITION_UNKNOWN,
                1f,
                SystemClock.elapsedRealtime(),
            )

        if (state == AudioEngine.State.Failed) {
            builder.setErrorMessage(
                PlaybackStateCompat.ERROR_CODE_UNKNOWN_ERROR,
                failure ?: "playback failed",
            )
        }
        session.setPlaybackState(builder.build())
    }

    fun release() {
        session.isActive = false
        session.release()
    }

    private companion object {
        const val TAG = "ausha"
    }
}
