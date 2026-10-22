package dev.empathic.clax_app

import android.app.NotificationManager
import android.content.Context
import android.media.AudioManager
import android.media.ToneGenerator
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

/**
 * Serves `dev.empathic.clax/cues` for `ChannelVoiceCues` in
 * `lib/platform/voice_cues.dart`: `play` {cue: "start"|"stop"} vibrates and
 * plays a tone, and answers whether the tone played.
 *
 * - Start: a heavy click ([VibrationEffect.EFFECT_HEAVY_CLICK]) and
 *   [ToneGenerator.TONE_PROP_BEEP], one 40 ms burst of 400 + 1200 Hz. It
 *   stays short: it plays while the recognizer opens the microphone.
 * - Stop: a double click ([VibrationEffect.EFFECT_DOUBLE_CLICK]) and
 *   [ToneGenerator.TONE_PROP_ACK] played whole, two 100 ms bursts of
 *   1200 Hz 100 ms apart.
 *
 * The vibration goes through the [Vibrator] (VIBRATE permission), so it is
 * felt even with system touch feedback off; a device without predefined
 * effects gets one-shot or waveform equivalents. No tone plays while the
 * ringer is on silent or vibrate, the notification volume is zero, or Do
 * Not Disturb holds back notifications; the vibration still happens.
 */
class CuesChannel(private val context: Context, messenger: BinaryMessenger) :
    MethodChannel.MethodCallHandler {
    private val handler = Handler(Looper.getMainLooper())

    init {
        MethodChannel(messenger, NAME).setMethodCallHandler(this)
    }

    private class Cue(
        val tone: Int,
        val toneMs: Int,
        val effect: Int,
        val fallback: () -> VibrationEffect,
    )

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        if (call.method != "play") {
            result.notImplemented()
            return
        }
        val cue = when (call.argument<String>("cue")) {
            "start" -> START
            "stop" -> STOP
            else -> {
                result.error("bad_cue", "cue must be start or stop", null)
                return
            }
        }
        vibrate(cue)
        result.success(!silenced() && play(cue))
    }

    private fun vibrator(): Vibrator? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            context.getSystemService(VibratorManager::class.java)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            context.getSystemService(Vibrator::class.java)
        }

    private fun vibrate(cue: Cue) {
        val v = vibrator() ?: return
        if (!v.hasVibrator()) return
        val predefined = Build.VERSION.SDK_INT < Build.VERSION_CODES.R ||
            v.areAllEffectsSupported(cue.effect) != Vibrator.VIBRATION_EFFECT_SUPPORT_NO
        try {
            v.vibrate(
                if (predefined) VibrationEffect.createPredefined(cue.effect) else cue.fallback(),
            )
        } catch (e: RuntimeException) {
            // A vibrator that refuses the effect: the tone is the cue.
        }
    }

    private fun silenced(): Boolean {
        val audio = context.getSystemService(AudioManager::class.java)
        if (audio == null || audio.ringerMode != AudioManager.RINGER_MODE_NORMAL) return true
        if (audio.getStreamVolume(AudioManager.STREAM_NOTIFICATION) == 0) return true
        val nm = context.getSystemService(NotificationManager::class.java)
        return nm != null &&
            nm.currentInterruptionFilter > NotificationManager.INTERRUPTION_FILTER_ALL
    }

    private fun play(cue: Cue): Boolean {
        val generator = try {
            ToneGenerator(AudioManager.STREAM_NOTIFICATION, VOLUME)
        } catch (e: RuntimeException) {
            // No audio output can be had right now.
            return false
        }
        val started = generator.startTone(cue.tone, cue.toneMs)
        // Released well after the longest tone, so none is cut short.
        handler.postDelayed({ generator.release() }, RELEASE_AFTER_MS)
        return started
    }

    companion object {
        const val NAME = "dev.empathic.clax/cues"
        private const val VOLUME = 80
        private const val RELEASE_AFTER_MS = 500L

        private val START = Cue(
            tone = ToneGenerator.TONE_PROP_BEEP,
            toneMs = 60,
            effect = VibrationEffect.EFFECT_HEAVY_CLICK,
            fallback = { VibrationEffect.createOneShot(40, VibrationEffect.DEFAULT_AMPLITUDE) },
        )
        private val STOP = Cue(
            tone = ToneGenerator.TONE_PROP_ACK,
            toneMs = 320,
            effect = VibrationEffect.EFFECT_DOUBLE_CLICK,
            fallback = { VibrationEffect.createWaveform(longArrayOf(0, 30, 90, 30), -1) },
        )
    }
}
