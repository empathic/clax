package dev.empathic.clax_app

import android.app.NotificationManager
import android.content.Context
import android.media.AudioManager
import android.media.ToneGenerator
import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

/**
 * Serves `dev.empathic.clax/cues` for `ChannelVoiceCues` in
 * `lib/platform/voice_cues.dart`: `play` {cue: "start"|"stop"} plays a
 * 120 ms tone on the notification stream ([ToneGenerator.TONE_PROP_BEEP]
 * to start, [ToneGenerator.TONE_PROP_ACK] to stop) and answers whether it
 * played. Nothing plays while the ringer is on silent or vibrate, or while
 * Do Not Disturb holds back notifications; the Dart side has vibrated
 * already.
 */
class CuesChannel(private val context: Context, messenger: BinaryMessenger) :
    MethodChannel.MethodCallHandler {
    private val handler = Handler(Looper.getMainLooper())

    init {
        MethodChannel(messenger, NAME).setMethodCallHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        if (call.method != "play") {
            result.notImplemented()
            return
        }
        val tone = when (call.argument<String>("cue")) {
            "start" -> ToneGenerator.TONE_PROP_BEEP
            "stop" -> ToneGenerator.TONE_PROP_ACK
            else -> {
                result.error("bad_cue", "cue must be start or stop", null)
                return
            }
        }
        result.success(!silenced() && play(tone))
    }

    private fun silenced(): Boolean {
        val audio = context.getSystemService(AudioManager::class.java)
        if (audio == null || audio.ringerMode != AudioManager.RINGER_MODE_NORMAL) return true
        if (audio.getStreamVolume(AudioManager.STREAM_NOTIFICATION) == 0) return true
        val nm = context.getSystemService(NotificationManager::class.java)
        return nm != null &&
            nm.currentInterruptionFilter > NotificationManager.INTERRUPTION_FILTER_ALL
    }

    private fun play(tone: Int): Boolean {
        val generator = try {
            ToneGenerator(AudioManager.STREAM_NOTIFICATION, VOLUME)
        } catch (e: RuntimeException) {
            // No audio output can be had right now.
            return false
        }
        val started = generator.startTone(tone, DURATION_MS)
        // Released after the tone, so it is never cut short.
        handler.postDelayed({ generator.release() }, DURATION_MS + RELEASE_SLACK_MS)
        return started
    }

    companion object {
        const val NAME = "dev.empathic.clax/cues"
        private const val DURATION_MS = 120
        private const val RELEASE_SLACK_MS = 200L
        private const val VOLUME = 80
    }
}
