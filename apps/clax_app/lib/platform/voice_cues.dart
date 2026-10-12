import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

import 'settings_store.dart';
import 'voice.dart';

/// What the person feels and hears as dictation starts and stops, so a
/// listen that began on its own is never a surprise and its end is clear.
abstract interface class VoiceCues {
  /// A listen began.
  Future<void> started();

  /// A listen ended: stopped, finished, failed or dropped.
  Future<void> stopped();
}

/// Cues that do nothing.
class NoVoiceCues implements VoiceCues {
  const NoVoiceCues();

  @override
  Future<void> started() async {}

  @override
  Future<void> stopped() async {}
}

/// [VoiceCues] as a vibration and a short tone.
///
/// The tone (and, with [platformHaptics], the vibration) is `play`
/// `{cue: "start"|"stop"}` on [channel] (`dev.empathic.clax/cues`), served
/// by `CuesChannel.kt` on Android and `MainFlutterWindow.swift` on macOS;
/// the platform plays no tone while the device is silenced. Without
/// [platformHaptics] the vibration is `HapticFeedback.mediumImpact` to
/// start and `HapticFeedback.lightImpact` to stop, sent first. A platform
/// without the channel only vibrates. Each cue reads [enabled] (the
/// person's "Dictation sounds and vibration") and does nothing when it
/// answers false. A cue never throws.
class ChannelVoiceCues implements VoiceCues {
  ChannelVoiceCues({
    required this.enabled,
    this.channel = defaultChannel,
    bool? platformHaptics,
  }) : platformHaptics = platformHaptics ?? Platform.isAndroid;

  static const defaultChannel = MethodChannel('dev.empathic.clax/cues');

  final bool Function() enabled;
  final MethodChannel channel;

  /// Whether `play` vibrates too, so `HapticFeedback` is not used. True on
  /// Android, whose `HapticFeedback` effects are faint, alike, and dropped
  /// when the person turned touch feedback off.
  final bool platformHaptics;

  @override
  Future<void> started() => _cue('start', HapticFeedback.mediumImpact);

  @override
  Future<void> stopped() => _cue('stop', HapticFeedback.lightImpact);

  Future<void> _cue(String cue, Future<void> Function() haptic) async {
    if (!enabled()) return;
    if (!platformHaptics) {
      try {
        await haptic();
      } on PlatformException {
        // No vibrator: the tone still plays.
      }
    }
    try {
      await channel.invokeMethod<void>('play', {'cue': cue});
    } on PlatformException {
      // The tone could not play; the haptic was the cue.
    } on MissingPluginException {
      // This platform has no tones.
    }
  }
}

/// The app's [VoiceInput]: [inner] with the person's dictation settings
/// applied. Every listen is cued through [cues] (one start as it begins,
/// one stop as its stream ends or its listener cancels), so every surface
/// that dictates through it is cued without code of its own. [autoStart]
/// tells surfaces that may start listening on their own (the New sheet)
/// whether to; read it with [autoStartOf].
class DictationVoice implements VoiceInput {
  DictationVoice(this.inner, {required this._cues, required this._autoStart});

  /// [inner] under [store]'s settings as they are saved: "Start dictation
  /// automatically" ([AppSettings.dictateComments]) and "Dictation sounds
  /// and vibration" ([AppSettings.dictationCues], given to [cues] as
  /// `enabled`; [ChannelVoiceCues] by default).
  factory DictationVoice.forSettings(
    VoiceInput inner,
    SettingsStore store, {
    VoiceCues Function(bool Function() enabled) cues = _channelCues,
  }) => DictationVoice(
    inner,
    cues: cues(() => store.last.dictationCues),
    autoStart: () => store.last.dictateComments,
  );

  static VoiceCues _channelCues(bool Function() enabled) =>
      ChannelVoiceCues(enabled: enabled);

  final VoiceInput inner;
  final VoiceCues _cues;
  final bool Function() _autoStart;

  /// Ends the running listen's cue, if any.
  void Function()? _endCurrent;

  /// Whether a surface should start dictation on its own.
  bool get autoStart => _autoStart();

  @override
  Future<VoiceAvailability> availability() => inner.availability();

  @override
  Future<bool> openPermissionSettings() => inner.openPermissionSettings();

  @override
  Future<void> stop() => inner.stop();

  @override
  Stream<VoiceResult> listen({String? locale}) {
    // A new listen replaces the running one: its stop cue comes first.
    _endCurrent?.call();
    var ended = false;
    late final void Function() end;
    end = () {
      if (ended) return;
      ended = true;
      if (identical(_endCurrent, end)) _endCurrent = null;
      unawaited(_cues.stopped());
    };
    _endCurrent = end;
    unawaited(_cues.started());

    final heard = inner.listen(locale: locale);
    StreamSubscription<VoiceResult>? sub;
    late final StreamController<VoiceResult> out;
    out = StreamController<VoiceResult>(
      onListen: () {
        sub = heard.listen(
          out.add,
          onError: out.addError,
          onDone: () {
            end();
            out.close();
          },
        );
      },
      onCancel: () {
        end();
        return sub?.cancel();
      },
    );
    return out.stream;
  }
}

/// Whether a surface dictating through [voice] should start listening on
/// its own: [DictationVoice.autoStart], and yes for any other voice.
bool autoStartOf(VoiceInput voice) =>
    voice is DictationVoice ? voice.autoStart : true;
