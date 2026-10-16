import 'dart:async';

import 'package:clax_app/platform/voice.dart';

/// A [VoiceInput] for tests: a fixed availability, and a script of words
/// each [listen] plays back, every entry partial except the last, which is
/// final. With [hold] set, a listen plays its words and then stays open
/// until [stop], as a recognizer waiting for more speech does. With [atStop]
/// set, [stop] delivers those words as a final result before the listen
/// ends, as a recognizer finishing the speech it heard does.
class FakeVoice implements VoiceInput {
  FakeVoice({
    this.script = const [],
    VoiceAvailability? availability,
    this.hold = false,
    this.atStop,
  }) : availabilityNow = availability ?? const VoiceAvailable();

  /// A voice that is turned off with [reason].
  FakeVoice.denied(String reason) : this(availability: VoiceDenied(reason));

  /// A voice the platform does not have.
  FakeVoice.unsupported(String reason)
    : this(availability: VoiceUnsupported(reason));

  final List<String> script;
  final bool hold;
  final String? atStop;

  /// What [availability] answers; a test may change it, as the person
  /// granting access in system settings does.
  VoiceAvailability availabilityNow;

  /// How many times [openPermissionSettings] was called.
  int settingsOpened = 0;

  /// How many times [listen] and [stop] were called.
  int listens = 0;
  int stops = 0;

  StreamController<VoiceResult>? _current;

  /// Whether a listen is running.
  bool get listening => _current != null;

  @override
  Future<VoiceAvailability> availability() async => availabilityNow;

  @override
  Future<bool> openPermissionSettings() async {
    settingsOpened++;
    return true;
  }

  @override
  Stream<VoiceResult> listen({String? locale}) {
    listens++;
    final c = StreamController<VoiceResult>();
    _current = c;
    () async {
      for (var i = 0; i < script.length; i++) {
        await Future<void>.delayed(Duration.zero);
        if (c.isClosed) return;
        c.add(VoiceResult(script[i], isFinal: !hold && i == script.length - 1));
      }
      if (!hold) await _end(c);
    }();
    return c.stream;
  }

  @override
  Future<void> stop() async {
    stops++;
    final c = _current;
    if (c == null) return;
    if (atStop case final words? when !c.isClosed) {
      c.add(VoiceResult(words, isFinal: true));
    }
    await _end(c);
  }

  Future<void> _end(StreamController<VoiceResult> c) async {
    if (identical(_current, c)) _current = null;
    if (!c.isClosed) await c.close();
  }
}
