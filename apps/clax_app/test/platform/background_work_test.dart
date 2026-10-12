import 'dart:async';

import 'package:clax_app/platform/background_work.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// Records every call, as `method(args…)`.
class RecordingWork implements BackgroundWork {
  final calls = <String>[];

  @override
  Future<bool> start(String title, String step) async {
    calls.add('start($title, $step)');
    return true;
  }

  @override
  Future<void> update(String title, String step) async =>
      calls.add('update($title, $step)');

  @override
  Future<void> stop() async => calls.add('stop');

  @override
  Future<void> ready(String title, String text, String artifactId) async =>
      calls.add('ready($title, $text, $artifactId)');

  @override
  Future<void> failed(
    String title,
    String reason, {
    String? artifactId,
  }) async => calls.add('failed($title, $reason, $artifactId)');

  @override
  Future<bool> requestPermission() async {
    calls.add('requestPermission');
    return true;
  }

  @override
  Future<String?> launchArtifact() async => null;

  @override
  Stream<String> get opened => const Stream.empty();
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  const channel = MethodChannel(ChannelBackgroundWork.channelName);

  group('BackgroundRuns', () {
    test('one run starts, steps and stops the service', () {
      final w = RecordingWork();
      final runs = BackgroundRuns(w, foreground: () => true);
      final r = runs.begin('Making “Runs”…', 'Starting');
      expect(runs.busy, isTrue);
      runs.step(r, 'Writing');
      runs.step(r, 'Writing');
      runs.end(r, const RunReady('a1', 'Runs is ready', 'Tap to open it'));
      runs.step(r, 'late');
      runs.end(r, const RunQuiet());
      expect(runs.busy, isFalse);
      // In the foreground the screen says it; nothing is posted.
      expect(w.calls, [
        'start(Making “Runs”…, Starting)',
        'update(Making “Runs”…, Writing)',
        'stop',
      ]);
    });

    test('in the background, ready and failed runs post their news', () {
      final w = RecordingWork();
      var front = true;
      final runs = BackgroundRuns(w, foreground: () => front);
      final a = runs.begin('A', 's');
      final b = runs.begin('B', 's');
      front = false;
      runs.end(a, const RunFailed("Couldn't make it", 'no key'));
      runs.end(b, const RunReady('b1', 'B is ready', 'Tap to open it'));
      expect(w.calls, [
        'start(A, s)',
        'update(B, s)',
        "failed(Couldn't make it, no key, null)",
        'stop',
        'ready(B is ready, Tap to open it, b1)',
      ]);
    });

    test('the notification follows the newest run still going', () {
      final w = RecordingWork();
      final runs = BackgroundRuns(w, foreground: () => true);
      final a = runs.begin('A', 'a1');
      final b = runs.begin('B', 'b1');
      runs.step(a, 'a2'); // not shown: B is newer
      runs.end(b, const RunQuiet());
      runs.step(a, 'a3');
      runs.end(a, const RunQuiet());
      expect(w.calls, [
        'start(A, a1)',
        'update(B, b1)',
        'update(A, a2)',
        'update(A, a3)',
        'stop',
      ]);
    });
  });

  group('ChannelBackgroundWork', () {
    late List<MethodCall> calls;
    setUp(() {
      calls = [];
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call);
        return switch (call.method) {
          'start' || 'requestPermission' => true,
          'launchArtifact' => 'a7',
          _ => null,
        };
      });
    });
    tearDown(() => messenger.setMockMethodCallHandler(channel, null));

    test('sends each call with its arguments', () async {
      final w = ChannelBackgroundWork();
      expect(await w.start('T', 'S'), isTrue);
      await w.update('T', 'S2');
      await w.ready('R', 'Tap', 'a1');
      await w.failed('F', 'why');
      await w.stop();
      expect(await w.requestPermission(), isTrue);
      expect(await w.launchArtifact(), 'a7');
      expect(
        [for (final c in calls) '${c.method} ${c.arguments}'],
        [
          'start {title: T, text: S}',
          'update {title: T, text: S2}',
          'ready {title: R, text: Tap, artifact: a1}',
          'failed {title: F, text: why, artifact: null}',
          'stop null',
          'requestPermission null',
          'launchArtifact null',
        ],
      );
    });

    test('a tapped notification arrives on opened', () async {
      final w = ChannelBackgroundWork();
      final got = <String>[];
      final sub = w.opened.listen(got.add);
      await messenger.handlePlatformMessage(
        ChannelBackgroundWork.channelName,
        const StandardMethodCodec().encodeMethodCall(
          const MethodCall('opened', {'artifact': 'a9'}),
        ),
        (_) {},
      );
      await pumpEventQueue();
      expect(got, ['a9']);
      await sub.cancel();
    });

    test('a platform without the channel answers false and null', () async {
      messenger.setMockMethodCallHandler(channel, null);
      final w = ChannelBackgroundWork();
      expect(await w.start('T', 'S'), isFalse);
      await w.stop();
      expect(await w.requestPermission(), isFalse);
      expect(await w.launchArtifact(), isNull);
    });

    test('a platform error is swallowed', () async {
      messenger.setMockMethodCallHandler(
        channel,
        (call) async => throw PlatformException(code: 'denied'),
      );
      expect(await ChannelBackgroundWork().start('T', 'S'), isFalse);
    });
  });
}
