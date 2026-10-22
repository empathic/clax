import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:speech_to_text/speech_recognition_error.dart';
import 'package:speech_to_text/speech_recognition_result.dart';
import 'package:speech_to_text/speech_to_text.dart';

/// Whether dictation can run, and why not when it cannot.
sealed class VoiceAvailability {
  const VoiceAvailability();
}

/// Dictation can run.
class VoiceAvailable extends VoiceAvailability {
  const VoiceAvailable();
}

/// The person turned the microphone or speech recognition off.
class VoiceDenied extends VoiceAvailability {
  const VoiceDenied(this.reason);

  /// Fit to show the person, for example as the mic button's tooltip.
  final String reason;
}

/// The platform has no speech recognizer the app can use.
class VoiceUnsupported extends VoiceAvailability {
  const VoiceUnsupported(this.reason);

  /// Fit to show the person.
  final String reason;
}

/// Words heard so far in one listen. Partial results replace each other;
/// the final one ends the listen.
class VoiceResult {
  const VoiceResult(this.words, {required this.isFinal});

  /// Everything heard in this listen, not only the newest words.
  final String words;
  final bool isFinal;

  @override
  String toString() => 'VoiceResult($words, final: $isFinal)';
}

/// A recognizer error during a listen, delivered on [VoiceInput.listen]'s
/// stream before it closes.
class VoiceError implements Exception {
  const VoiceError(this.code);

  /// The recognizer's own report, for the log: `speech_to_text`'s error
  /// code (`error_no_match`, `error_network`, …) or the text of the
  /// exception a listen failed to start with.
  final String code;

  /// One plain line for the person ([voiceErrorLine]).
  String get message => voiceErrorLine(code);

  @override
  String toString() => 'VoiceError: $code';
}

/// The line a dictation error of [code] reads as: no speech heard, no
/// permission, no network, the recognizer busy, or else that dictation
/// stopped. [code] is `speech_to_text`'s error code or an exception's
/// text, matched without regard to case.
String voiceErrorLine(String code) {
  final c = code.toLowerCase();
  if (c.contains('no_match') || c.contains('speech_timeout')) {
    return 'Dictation didn’t hear anything. Try again.';
  }
  if (c.contains('permission')) {
    return 'Dictation needs microphone access. Turn it on in Settings.';
  }
  if (c.contains('network') || c.contains('server_disconnected')) {
    return 'Dictation needs the internet. Check your connection and try '
        'again.';
  }
  if (c.contains('busy') ||
      c.contains('too_many_requests') ||
      c.contains('multiplerequests')) {
    return 'Dictation is busy. Try again in a moment.';
  }
  return 'Dictation stopped. Try again.';
}

/// Speech to text for the New sheet and for comments.
abstract interface class VoiceInput {
  /// Whether dictation can run. May ask the person for permission the first
  /// time.
  Future<VoiceAvailability> availability();

  /// Listens once: partial results, then a final one, then the stream
  /// closes. It also closes after [stop], once the recognizer has given its
  /// last words, and after a [VoiceError].
  Stream<VoiceResult> listen({String? locale});

  /// Ends the running listen, if any.
  Future<void> stop();

  /// Opens the system settings where the person can give the app the
  /// microphone and speech access [VoiceDenied] reports. Returns whether a
  /// settings screen was opened.
  Future<bool> openPermissionSettings();
}

/// [VoiceInput] over the `speech_to_text` plugin. It asks for on-device
/// recognition and, when that fails before any words are heard (no
/// on-device model for the language), listens again once with the
/// platform's default recognizer.
class SpeechToTextVoice implements VoiceInput {
  SpeechToTextVoice({SpeechToText? speech})
    : _speech = speech ?? SpeechToText();

  final SpeechToText _speech;
  Future<VoiceAvailability>? _availability;
  _Listen? _current;

  /// Whether this platform has a recognizer the plugin drives.
  static bool get platformSupported =>
      Platform.isAndroid || Platform.isMacOS || Platform.isIOS;

  @override
  Future<VoiceAvailability> availability() => _availability ??= _init();

  Future<VoiceAvailability> _init() async {
    if (!platformSupported) {
      return const VoiceUnsupported('Dictation is not available here');
    }
    try {
      final ok = await _speech.initialize(
        onError: (e) => _current?.error(e),
        onStatus: (s) => _current?.status(s),
      );
      if (ok) return const VoiceAvailable();
      final permitted = await _speech.hasPermission;
      // A refusal is remembered by the plugin; ask again next time.
      _availability = null;
      return permitted
          ? const VoiceUnsupported('Speech recognition is not available')
          : const VoiceDenied(
              'Microphone or speech recognition access is turned off',
            );
    } on PlatformException catch (e) {
      _availability = null;
      return VoiceUnsupported(
        e.message ?? 'Speech recognition failed to start',
      );
    }
  }

  @override
  Stream<VoiceResult> listen({String? locale}) {
    _current?.close();
    final l = _Listen(this, locale);
    _current = l;
    l.start(onDevice: true);
    return l.controller.stream;
  }

  /// The app's MethodChannel to its Android activity
  /// (`android/app/src/main/kotlin/.../MainActivity.kt`).
  static const _native = MethodChannel('dev.empathic.clax/native');

  /// macOS: the Privacy & Security pane's Microphone list.
  static const _macMicrophonePane =
      'x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone';

  @override
  Future<bool> openPermissionSettings() async {
    try {
      if (Platform.isAndroid) {
        return await _native.invokeMethod<bool>('openAppSettings') ?? false;
      }
      if (Platform.isMacOS) {
        final r = await Process.run('/usr/bin/open', [_macMicrophonePane]);
        return r.exitCode == 0;
      }
    } on PlatformException {
      return false;
    } on ProcessException {
      return false;
    }
    return false;
  }

  @override
  Future<void> stop() async {
    final l = _current;
    if (l == null) return;
    l.stopping = true;
    await _speech.stop();
  }
}

/// One [SpeechToTextVoice.listen], from its first words to its stream's
/// close.
class _Listen {
  _Listen(this.voice, this.locale);

  final SpeechToTextVoice voice;
  final String? locale;
  final controller = StreamController<VoiceResult>();
  bool onDevice = true;
  bool heard = false;
  bool stopping = false;

  /// Between a failed on-device listen and the default recognizer's
  /// `listening`: the failed listen's `done` must not end this one.
  bool restarting = false;

  Future<void> start({required bool onDevice}) async {
    this.onDevice = onDevice;
    try {
      await voice._speech.listen(
        onResult: _result,
        listenOptions: SpeechListenOptions(
          onDevice: onDevice,
          partialResults: true,
          cancelOnError: true,
          listenMode: ListenMode.dictation,
          localeId: locale,
        ),
      );
    } catch (e) {
      controller.addError(VoiceError('$e'));
      close();
    }
  }

  void _result(SpeechRecognitionResult r) {
    if (controller.isClosed) return;
    heard = heard || r.recognizedWords.isNotEmpty;
    controller.add(VoiceResult(r.recognizedWords, isFinal: r.finalResult));
    if (r.finalResult) close();
  }

  void error(SpeechRecognitionError e) {
    if (controller.isClosed) return;
    if (onDevice && !heard && !stopping) {
      // No on-device model (or it refused): try the default recognizer.
      restarting = true;
      start(onDevice: false);
      return;
    }
    if ((e.errorMsg == 'error_no_match' ||
            e.errorMsg == 'error_speech_timeout') &&
        (heard || stopping)) {
      close();
      return;
    }
    controller.addError(VoiceError(e.errorMsg));
    close();
  }

  void status(String s) {
    if (s == SpeechToText.listeningStatus) restarting = false;
    if (s == SpeechToText.doneStatus && !restarting) close();
  }

  void close() {
    if (identical(voice._current, this)) voice._current = null;
    if (!controller.isClosed) controller.close();
  }
}
