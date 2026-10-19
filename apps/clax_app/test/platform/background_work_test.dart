import 'dart:async';

import 'package:clax_app/platform/background_work.dart';
import 'package:clax_app/platform/local_notifications.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_notifications.dart';

/// Records every call, as `method(args…)`.
class RecordingWork implements BackgroundWork {
  final calls = <String>[];

  /// What the next starts answer.
  var starts = true;
  final stops = StreamController<void>.broadcast();

  @override
  Stream<void> get stopped => stops.stream;

  @override
  Future<bool> start(String title, String step) async {
    calls.add('start($title, $step)');
    return starts;
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

  @override
  bool get pausesRuns => false;

  @override
  Future<void> paused(String title, String? artifactId) async =>
      calls.add('paused($title, $artifactId)');

  @override
  Future<void> pauseDone() async => calls.add('pauseDone');
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
      runs.step(a, 'a3', title: 'A2');
      runs.end(a, const RunQuiet());
      expect(w.calls, [
        'start(A, a1)',
        'update(B, b1)',
        'update(A, a2)',
        'update(A, a3)',
        'update(A2, a3)',
        'stop',
      ]);
    });
    test('runs of several apps at once keep one service up until the last '
        'ends, in any order', () {
      final w = RecordingWork();
      final runs = BackgroundRuns(w, foreground: () => true);
      final a = runs.begin('A', 'a');
      final b = runs.begin('B', 'b');
      final c = runs.begin('C', 'c');
      runs.end(b, const RunQuiet());
      runs.end(c, const RunQuiet());
      expect(runs.busy, isTrue);
      expect(w.calls.where((c) => c == 'stop'), isEmpty);
      final d = runs.begin('D', 'd');
      runs.end(a, const RunQuiet());
      runs.end(d, const RunQuiet());
      expect(runs.busy, isFalse);
      expect(w.calls, [
        'start(A, a)',
        'update(B, b)',
        'update(C, c)',
        'update(A, a)',
        'update(D, d)',
        'stop',
      ]);
    });
  });

  group('BackgroundRuns and the service', () {
    test('a refused start is tried again by the next run', () async {
      final w = RecordingWork()..starts = false;
      final runs = BackgroundRuns(w, foreground: () => true);
      final a = runs.begin('A', 's');
      await pumpEventQueue();
      runs.step(a, 's2'); // no service: nothing to update
      w.starts = true;
      runs.begin('B', 's');
      expect(w.calls, ['start(A, s)', 'start(B, s)']);
    });

    test('coming to the foreground restarts a refused service while runs '
        'go on', () async {
      final w = RecordingWork()..starts = false;
      final runs = BackgroundRuns(w, foreground: () => true);
      final a = runs.begin('A', 's');
      await pumpEventQueue();
      w.starts = true;
      runs.onForeground();
      await pumpEventQueue();
      runs.onForeground(); // already running: nothing
      runs.step(a, 's2');
      runs.end(a, const RunQuiet());
      runs.onForeground(); // nothing runs: nothing
      expect(w.calls, ['start(A, s)', 'start(A, s)', 'update(A, s2)', 'stop']);
    });

    test('a service that stopped on its own is started again', () async {
      final w = RecordingWork();
      final runs = BackgroundRuns(w, foreground: () => true);
      final a = runs.begin('A', 's');
      await pumpEventQueue();
      w.stops.add(null);
      await pumpEventQueue();
      runs.step(a, 's2');
      runs.begin('B', 's');
      expect(w.calls, ['start(A, s)', 'start(B, s)']);
    });

    test('a late refusal of an older start does not mark a newer one '
        'down', () async {
      final w = RecordingWork();
      final gate = Completer<bool>();
      var first = true;
      final runs = BackgroundRuns(
        _GatedStart(w, () {
          if (!first) return Future.value(true);
          first = false;
          return gate.future;
        }),
        foreground: () => true,
      );
      final a = runs.begin('A', 's');
      runs.end(a, const RunQuiet());
      final b = runs.begin('B', 's');
      gate.complete(false);
      await pumpEventQueue();
      runs.step(b, 's2');
      expect(w.calls, ['start(A, s)', 'stop', 'start(B, s)', 'update(B, s2)']);
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

    test('a stop by the service arrives on stopped', () async {
      final w = ChannelBackgroundWork();
      var stops = 0;
      final sub = w.stopped.listen((_) => stops++);
      await messenger.handlePlatformMessage(
        ChannelBackgroundWork.channelName,
        const StandardMethodCodec().encodeMethodCall(
          const MethodCall('stopped'),
        ),
        (_) {},
      );
      await pumpEventQueue();
      expect(stops, 1);
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
  group('pausesRuns', () {
    test('only iOS pauses the runs on stopped', () async {
      final hub = await NotificationHub.init(plugin: FakeNotificationsPlugin());
      expect(IosBackgroundWork(hub).pausesRuns, isTrue);
      expect(ChannelBackgroundWork().pausesRuns, isFalse);
      expect(const NoBackgroundWork().pausesRuns, isFalse);
    });
  });

  group('IosBackgroundWork', () {
    late List<MethodCall> calls;
    late FakeNotificationsPlugin plugin;
    late NotificationHub hub;
    setUp(() async {
      calls = [];
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call);
        return call.method == 'start' ? true : null;
      });
      plugin = FakeNotificationsPlugin();
      hub = await NotificationHub.init(plugin: plugin);
    });
    tearDown(() => messenger.setMockMethodCallHandler(channel, null));

    test('the paused notice is titled by the app, and a second replaces '
        'the first', () async {
      final w = IosBackgroundWork(hub);
      await w.paused('Groceries', 'a1');
      await w.paused('Runs', 'a2');
      expect(plugin.shown, [
        '${IosBackgroundWork.pausedId}|Groceries|'
            'Paused — open Clax to keep building.|run:a1',
        '${IosBackgroundWork.pausedId}|Runs|'
            'Paused — open Clax to keep building.|run:a2',
      ]);
    });

    test('ready and failed post through the hub, keyed by the artifact, '
        'and never reach the channel', () async {
      final w = IosBackgroundWork(hub);
      await w.ready('Groceries is ready', 'Tap to open it', 'a1');
      await w.failed("Couldn't change Groceries", 'no key', artifactId: 'a1');
      await w.failed("Couldn't make it", 'no key');
      final id = IosBackgroundWork.newsId('a1');
      expect(plugin.shown, [
        '$id|Groceries is ready|Tap to open it|run:a1',
        "$id|Couldn't change Groceries|no key|run:a1",
        "${IosBackgroundWork.newsId(null)}|Couldn't make it|no key|run:",
      ]);
      expect(calls, isEmpty);
    });

    test('a notice ID is the same in every process', () {
      expect(IosBackgroundWork.newsId('a1'), IosBackgroundWork.newsId('a1'));
      expect(IosBackgroundWork.newsId('a1'), isNot(IosBackgroundWork.pausedId));
      expect(IosBackgroundWork.newsId('a1'), lessThan(1 << 20));
    });

    test('taps on run notices open their artifact; one for no artifact '
        'opens nothing', () async {
      final w = IosBackgroundWork(hub);
      final got = <String>[];
      final sub = w.opened.listen(got.add);
      await hub.ready;
      plugin.tap('run:a1');
      plugin.tap('run:');
      plugin.tap('reminder:{}');
      await pumpEventQueue();
      expect(got, ['a1']);
      await sub.cancel();
    });

    test('the run notice that launched the app is its launch '
        'artifact', () async {
      plugin.launchedBy = 'run:a3';
      expect(await IosBackgroundWork(hub).launchArtifact(), 'a3');
    });

    test('start, update, stop and pauseDone go over the channel', () async {
      final w = IosBackgroundWork(hub);
      expect(await w.start('T', 'S'), isTrue);
      await w.update('T', 'S2');
      await w.stop();
      await w.pauseDone();
      expect(
        [for (final c in calls) '${c.method} ${c.arguments}'],
        [
          'start {title: T, text: S}',
          'update {title: T, text: S2}',
          'stop null',
          'pauseDone null',
        ],
      );
    });

    test('a stopped call from Swift arrives on stopped', () async {
      final w = IosBackgroundWork(hub);
      var stops = 0;
      final sub = w.stopped.listen((_) => stops++);
      await messenger.handlePlatformMessage(
        ChannelBackgroundWork.channelName,
        const StandardMethodCodec().encodeMethodCall(
          const MethodCall('stopped'),
        ),
        (_) {},
      );
      await pumpEventQueue();
      expect(stops, 1);
      await sub.cancel();
    });
  });
}

/// [RecordingWork] whose start answers [answer] instead.
class _GatedStart implements BackgroundWork {
  _GatedStart(this.inner, this.answer);
  final RecordingWork inner;
  final Future<bool> Function() answer;

  @override
  Future<bool> start(String title, String step) {
    inner.calls.add('start($title, $step)');
    return answer();
  }

  @override
  Future<void> update(String title, String step) => inner.update(title, step);
  @override
  Future<void> stop() => inner.stop();
  @override
  Future<void> ready(String title, String text, String artifactId) =>
      inner.ready(title, text, artifactId);
  @override
  Future<void> failed(String title, String reason, {String? artifactId}) =>
      inner.failed(title, reason, artifactId: artifactId);
  @override
  Future<bool> requestPermission() => inner.requestPermission();
  @override
  Future<String?> launchArtifact() => inner.launchArtifact();
  @override
  Stream<String> get opened => inner.opened;
  @override
  Stream<void> get stopped => inner.stopped;
  @override
  bool get pausesRuns => inner.pausesRuns;
  @override
  Future<void> paused(String title, String? artifactId) =>
      inner.paused(title, artifactId);
  @override
  Future<void> pauseDone() => inner.pauseDone();
}
