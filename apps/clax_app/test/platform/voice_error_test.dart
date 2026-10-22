import 'package:clax_app/platform/voice.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  const noSpeech = 'Dictation didn’t hear anything. Try again.';
  const permission =
      'Dictation needs microphone access. Turn it on in Settings.';
  const network =
      'Dictation needs the internet. Check your connection and try again.';
  const busy = 'Dictation is busy. Try again in a moment.';
  const other = 'Dictation stopped. Try again.';

  test('each common recognizer code reads as its own plain line', () {
    final lines = {
      'error_no_match': noSpeech,
      'error_speech_timeout': noSpeech,
      'error_insufficient_permissions': permission,
      'error_network': network,
      'error_network_timeout': network,
      'error_server_disconnected': network,
      'error_busy': busy,
      'error_too_many_requests': busy,
      'PlatformException(multipleRequests, already listening, null, null)':
          busy,
      'error_audio': other,
      'error_client': other,
    };
    for (final MapEntry(key: code, value: line) in lines.entries) {
      expect(VoiceError(code).message, line, reason: code);
    }
    expect({noSpeech, permission, network, busy, other}, hasLength(5));
  });

  test('the raw code stays for the log', () {
    const e = VoiceError('error_network');
    expect(e.code, 'error_network');
    expect('$e', 'VoiceError: error_network');
  });
}
