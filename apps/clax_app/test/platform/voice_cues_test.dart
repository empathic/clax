import 'dart:async';
import 'dart:io';

import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/platform/voice.dart';
import 'package:clax_app/platform/voice_cues.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

/// Records every cue, as `started` or `stopped`.
class RecordingCues implements VoiceCues {
  final calls = <String>[];

  @override
  Future<void> started() async => calls.add('started');

  @override
  Future<void> stopped() async => calls.add('stopped');
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('ChannelVoiceCues', () {
    const channel = MethodChannel('dev.empathic.clax/cues');
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    late List<String> log;

    setUp(() {
      log = [];
      messenger.setMockMethodCallHandler(SystemChannels.platform, (c) async {
        if (c.method == 'HapticFeedback.vibrate') {
          log.add('haptic ${c.arguments}');
        }
        return null;
      });
      messenger.setMockMethodCallHandler(channel, (c) async {
        log.add('${c.method} ${c.arguments}');
        return null;
      });
    });
    tearDown(() {
      messenger.setMockMethodCallHandler(SystemChannels.platform, null);
      messenger.setMockMethodCallHandler(channel, null);
    });

    test('start: a medium impact, then the start tone', () async {
      await ChannelVoiceCues(enabled: () => true).started();
      expect(log, [
        'haptic HapticFeedbackType.mediumImpact',
        'play {cue: start}',
      ]);
    });

    test('stop: a light impact, then the stop tone', () async {
      await ChannelVoiceCues(enabled: () => true).stopped();
      expect(log, [
        'haptic HapticFeedbackType.lightImpact',
        'play {cue: stop}',
      ]);
    });

    test(
      'where the platform vibrates (Android), only the channel is called',
      () async {
        final cues = ChannelVoiceCues(
          enabled: () => true,
          platformHaptics: true,
        );
        await cues.started();
        await cues.stopped();
        expect(log, ['play {cue: start}', 'play {cue: stop}']);
      },
    );

    test('by default the platform vibrates on Android only: an iOS host '
        'plays HapticFeedback (UIKit\'s impact generator), then the tone', () async {
      final ios = ChannelVoiceCues(enabled: () => true, isAndroid: () => false);
      final android = ChannelVoiceCues(enabled: () => true, isAndroid: () => true);
      expect(ios.platformHaptics, isFalse);
      expect(android.platformHaptics, isTrue);
      await ios.started();
      await android.started();
      expect(log, [
        'haptic HapticFeedbackType.mediumImpact',
        'play {cue: start}',
        'play {cue: start}',
      ]);
    });

    test('turned off, neither vibrates nor sounds', () async {
      final cues = ChannelVoiceCues(enabled: () => false);
      await cues.started();
      await cues.stopped();
      expect(log, isEmpty);
    });

    test('the setting is read at each cue', () async {
      var on = false;
      final cues = ChannelVoiceCues(enabled: () => on);
      await cues.started();
      on = true;
      await cues.stopped();
      expect(log, [
        'haptic HapticFeedbackType.lightImpact',
        'play {cue: stop}',
      ]);
    });

    test(
      'a platform without the channel still vibrates and never throws',
      () async {
        messenger.setMockMethodCallHandler(channel, null);
        await ChannelVoiceCues(enabled: () => true).started();
        messenger.setMockMethodCallHandler(channel, (c) async {
          throw PlatformException(code: 'no_audio');
        });
        await ChannelVoiceCues(enabled: () => true).stopped();
        expect(log, [
          'haptic HapticFeedbackType.mediumImpact',
          'haptic HapticFeedbackType.lightImpact',
        ]);
      },
    );
  });

  group('DictationVoice', () {
    test('cues the start of a listen and its end once', () async {
      final cues = RecordingCues();
      final voice = DictationVoice(
        FakeVoice(script: ['a', 'a run']),
        cues: cues,
        autoStart: () => true,
      );
      final heard = await voice.listen().map((r) => r.words).toList();
      expect(heard, ['a', 'a run']);
      expect(cues.calls, ['started', 'stopped']);
    });

    test('a listen stopped by the person ends with one stop cue', () async {
      final cues = RecordingCues();
      final inner = FakeVoice(script: ['a'], hold: true);
      final voice = DictationVoice(inner, cues: cues, autoStart: () => true);
      final done = Completer<void>();
      voice.listen().listen((_) {}, onDone: done.complete);
      expect(cues.calls, ['started']);
      await voice.stop();
      await done.future;
      expect(inner.stops, 1);
      expect(cues.calls, ['started', 'stopped']);
    });

    test('a cancelled listen ends with one stop cue', () async {
      final cues = RecordingCues();
      final inner = FakeVoice(script: ['a'], hold: true);
      final voice = DictationVoice(inner, cues: cues, autoStart: () => true);
      final sub = voice.listen().listen((_) {});
      await sub.cancel();
      await voice.stop();
      expect(cues.calls, ['started', 'stopped']);
    });

    test('a recognizer error is passed on and the listen still ends with a stop cue', () async {
      final cues = RecordingCues();
      final voice = DictationVoice(
        _ErrorVoice(),
        cues: cues,
        autoStart: () => true,
      );
      final errors = <Object>[];
      final done = Completer<void>();
      voice.listen().listen((_) {}, onError: errors.add, onDone: done.complete);
      await done.future;
      expect(errors.single, isA<VoiceError>());
      expect(cues.calls, ['started', 'stopped']);
    });

    test('passes availability and permission settings through', () async {
      final inner = FakeVoice.denied('off');
      final voice = DictationVoice(
        inner,
        cues: const NoVoiceCues(),
        autoStart: () => false,
      );
      expect(await voice.availability(), isA<VoiceDenied>());
      expect(await voice.openPermissionSettings(), isTrue);
      expect(inner.settingsOpened, 1);
      expect(voice.autoStart, isFalse);
    });

    test(
      'autoStartOf: a DictationVoice answers its setting, any other voice yes',
      () {
        var on = true;
        final voice = DictationVoice(
          FakeVoice(),
          cues: const NoVoiceCues(),
          autoStart: () => on,
        );
        expect(autoStartOf(voice), isTrue);
        on = false;
        expect(autoStartOf(voice), isFalse);
        expect(autoStartOf(FakeVoice()), isTrue);
      },
    );

    test(
      'the app voice reads both settings from the store as they are saved',
      () async {
        final dir = Directory.systemTemp.createTempSync('clax-cues-');
        addTearDown(() => dir.deleteSync(recursive: true));
        final store = SettingsStore(File('${dir.path}/settings.json'));
        final cues = RecordingCues();
        final voice = DictationVoice.forSettings(
          FakeVoice(),
          store,
          cues: (enabled) {
            expect(enabled(), isTrue);
            return cues;
          },
        );
        expect(voice.autoStart, isTrue);
        await store.save(AppSettings(dictateComments: false));
        expect(voice.autoStart, isFalse);
      },
    );
  });
}

/// A voice whose listen fails at once.
class _ErrorVoice extends FakeVoice {
  @override
  Stream<VoiceResult> listen({String? locale}) =>
      Stream.error(const VoiceError('error_audio'));
}
