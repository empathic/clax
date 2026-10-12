import 'dart:async';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_menu.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/app_surface.dart';
import 'package:clax_app/ui/comment_button.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

/// A clock that stands still: every gesture is within its time window.
class _Clock implements Clock {
  @override
  DateTime now() => DateTime.utc(2026, 10, 12);
  @override
  Future<void> delay(Duration d) async {}
}

/// Timings that log what the screen asked of them.
class _Timings implements Timings {
  final log = <String>[];
  final extras = <Map<String, Object?>>[];

  @override
  void start(String name) => log.add('start $name');

  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) {
    log.add('stop $name');
    extras.add(extra);
    return TimingRecord(
      name: name,
      elapsed: Duration.zero,
      at: DateTime.utc(2026),
      extra: extra,
    );
  }

  @override
  List<TimingRecord> recent(int n) => const [];
}

const _page = {'index.html': '<!doctype html><h1>Runs</h1>'};
final _base = Uri.parse('http://127.0.0.1:7999');

class _Rig {
  _Rig(this.api, this.aid, this.surface, this.voice, this.timings);

  final FakeClaxApi api;
  final String aid;
  final FakeAppSurface surface;
  final FakeVoice voice;
  final _Timings timings;
  final worker = StreamController<WorkerEvent>.broadcast(sync: true);

  Ready ready({int version = 3, int latest = 3}) => Ready(
    artifact: aid,
    version: version,
    latest: latest,
    file: 'index.html',
  );

  static const draft = StateMsg(
    commenting: true,
    draft: Draft(label: 'h1', text: '', clip: DraftClip.pending),
    busy: false,
  );
  static const noDraft = StateMsg(commenting: true, draft: null, busy: false);
}

/// An artifact with three versions, the last two published by the agent.
Future<(FakeClaxApi, String)> _seeded() async {
  final api = FakeClaxApi();
  final aid = api.seedArtifact('Runs', _page);
  await api.registerAppSession(installId: 'install', pid: 1);
  await api.publish(
    aid,
    PublishRequest(
      ifVersion: 1,
      note: 'Bigger title',
      files: {'index.html': '<h1 style="font-size:2em">Runs</h1>'},
    ),
  );
  await api.publish(
    aid,
    PublishRequest(
      ifVersion: 2,
      note: 'Weekly total',
      files: {'index.html': '<h1>Runs</h1><p>Week: 0 km</p>'},
    ),
  );
  api.calls.clear();
  return (api, aid);
}

void main() {
  /// A phone: 360 x 780 logical pixels at a device pixel ratio of 3, with
  /// a 24 px status bar.
  void phone(WidgetTester tester, {Size size = const Size(360, 780)}) {
    tester.view.devicePixelRatio = 3;
    tester.view.physicalSize = size * 3;
    tester.view.padding = const FakeViewPadding(top: 72);
    addTearDown(tester.view.reset);
  }

  Future<_Rig> pumpScreen(
    WidgetTester tester, {
    AppSettings? settings,
    FakeVoice? voice,
    Size size = const Size(360, 780),
    double? buttonDy,
    ValueChanged<double>? onButtonMoved,
    CommentsCommand? comments,
    Future<bool> Function(String aid, String tid)? onRetry,
  }) async {
    phone(tester, size: size);
    final (api, aid) = await _seeded();
    final rig = _Rig(
      api,
      aid,
      FakeAppSurface(),
      voice ?? FakeVoice(script: const ['make it', 'make it blue'], hold: true),
      _Timings(),
    );
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light),
        home: AppScreen(
          aid: aid,
          base: _base,
          api: api,
          surface: rig.surface,
          voice: rig.voice,
          settings: settings ?? AppSettings(),
          timings: rig.timings,
          worker: rig.worker.stream,
          clock: _Clock(),
          commentButtonDy: buttonDy,
          onCommentButtonMoved: onButtonMoved,
          comments: comments,
          onRetry: onRetry,
        ),
      ),
    );
    await tester.pump();
    return rig;
  }

  /// Frames enough for a restore and its list to finish while a spinner
  /// (which never settles) runs.
  Future<void> frames(WidgetTester tester) async {
    for (var i = 0; i < 10; i++) {
      await tester.pump(const Duration(milliseconds: 50));
    }
  }

  Future<void> openPanel(WidgetTester tester) async {
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('panel-handle')));
    await tester.pumpAndSettle();
  }

  Future<void> twoFingerSwipe(WidgetTester tester, double dx) async {
    final a = await tester.startGesture(const Offset(200, 300), pointer: 1);
    final b = await tester.startGesture(const Offset(200, 400), pointer: 2);
    for (var i = 1; i <= 3; i++) {
      await a.moveTo(Offset(200 + dx * i / 3, 300));
      await b.moveTo(Offset(200 + dx * i / 3, 400));
    }
    await a.up();
    await b.up();
    await tester.pumpAndSettle();
  }

  testWidgets('loads the app view and stays covered until the shell is ready', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    expect(rig.surface.loads, [
      Uri.parse('http://127.0.0.1:7999/a/${rig.aid}?app=1'),
    ]);
    expect(find.byKey(const ValueKey('app-cover')), findsOneWidget);
    rig.surface.push(rig.ready());
    await tester.pump();
    expect(find.byKey(const ValueKey('app-cover')), findsNothing);
    expect(find.byKey(const ValueKey('fake-surface')), findsOneWidget);
  });

  testWidgets('a base URL with a path keeps it', (tester) async {
    phone(tester);
    final (api, aid) = await _seeded();
    final surface = FakeAppSurface();
    await tester.pumpWidget(
      MaterialApp(
        home: AppScreen(
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999/clax/'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: _Timings(),
          worker: const Stream.empty(),
        ),
      ),
    );
    expect(
      surface.loads.single.toString(),
      'http://127.0.0.1:7999/clax/a/$aid?app=1',
    );
  });

  testWidgets(
    'every pointer down and up on the page is attested in page pixels',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await tester.tapAt(const Offset(100, 200));
      await tester.pump();
      final a = rig.surface.attests;
      expect(a, hasLength(2), reason: 'the down and the up');
      for (final p in a) {
        // The page starts below the 24 px status bar.
        expect(p.x, 100);
        expect(p.y, 176);
        expect(p.devicePixelRatio, 3);
      }
      expect(
        [for (final p in a) p.phase],
        [PointerPhase.down, PointerPhase.up],
      );
      expect(
        a[0].pointer,
        a[1].pointer,
        reason: 'the press and release of one pointer',
      );
      // A long press is attested too: down, then up after the hold.
      final g = await tester.startGesture(const Offset(50, 400));
      await tester.pump(const Duration(milliseconds: 600));
      await g.up();
      await tester.pump();
      expect(rig.surface.attests, hasLength(4));
      expect(rig.surface.hostCalls, isEmpty);
    },
  );

  testWidgets('a mouse on the page is not attested; a stylus is', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await tester.pump();
    // A mouse's input reaches the WebView itself, and script run during
    // its click clears the shell's activation in WebKit.
    await tester.tapAt(const Offset(100, 200), kind: PointerDeviceKind.mouse);
    await tester.pump();
    expect(rig.surface.attests, isEmpty);
    await tester.tapAt(const Offset(100, 200), kind: PointerDeviceKind.stylus);
    await tester.pump();
    expect(rig.surface.attests, hasLength(2));
  });

  testWidgets(
    'a two-finger swipe left opens the panel and comment mode; right closes both',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await twoFingerSwipe(tester, -120);
      expect(find.byKey(const ValueKey('app-panel')), findsOneWidget);
      expect(rig.surface.js, ['window.claxHost?.enterCommentMode()']);
      // The fingers were attested as well: two downs and two ups.
      expect(rig.surface.attests, hasLength(4));

      await twoFingerSwipe(tester, 120);
      expect(find.byKey(const ValueKey('app-panel')), findsNothing);
      expect(rig.surface.js, [
        'window.claxHost?.enterCommentMode()',
        'window.claxHost?.exitCommentMode()',
      ]);
    },
  );

  testWidgets('the edge handle opens the panel and back closes it', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await tester.pump();
    await openPanel(tester);
    expect(find.byKey(const ValueKey('app-panel')), findsOneWidget);
    expect(find.byKey(const ValueKey('panel-handle')), findsNothing);
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-panel')), findsNothing);
    expect(find.byType(AppScreen), findsOneWidget);
    expect(rig.surface.js.last, 'window.claxHost?.exitCommentMode()');
  });

  testWidgets(
    'on a narrow screen the panel takes the lower 45% and the page sits above it',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      final panel = tester.getRect(find.byKey(const ValueKey('app-panel')));
      final page = tester.getRect(find.byKey(const ValueKey('fake-surface')));
      expect(panel.height, closeTo(780 * 0.45, 1));
      expect(panel.bottom, 780);
      expect(page.bottom, closeTo(panel.top, 0.01));
    },
  );

  testWidgets('on a wide screen the panel is a 360 px side panel', (
    tester,
  ) async {
    final rig = await pumpScreen(tester, size: const Size(1200, 800));
    rig.surface.push(rig.ready());
    await openPanel(tester);
    final panel = tester.getRect(find.byKey(const ValueKey('app-panel')));
    expect(panel.width, 360);
    expect(panel.right, 1200);
    final page = tester.getRect(find.byKey(const ValueKey('fake-surface')));
    expect(page.right, closeTo(panel.left, 0.01));
  });

  testWidgets(
    'a pick shrinks the panel to a peek and dictates into the draft; the timing waits for Send',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      final full = tester.getRect(find.byKey(const ValueKey('app-panel')));
      rig.surface.push(_Rig.draft);
      await tester.pumpAndSettle();
      final peek = tester.getRect(find.byKey(const ValueKey('app-panel')));
      expect(peek.height, lessThan(full.height / 2));
      expect(find.text('Versions'), findsNothing);
      expect(rig.timings.log, isEmpty, reason: 'it starts at Send (spec 6.2)');
      expect(rig.voice.listens, 1);
      expect(
        rig.surface.js,
        containsAllInOrder([
          'window.claxHost?.setDraftText("make it")',
          'window.claxHost?.setDraftText("make it blue")',
        ]),
      );

      // The mic button stops and starts listening.
      await tester.tap(find.byKey(const ValueKey('dictate')));
      await tester.pumpAndSettle();
      expect(rig.voice.listening, isFalse);
      expect(rig.voice.stops, 1);
      await tester.tap(find.byKey(const ValueKey('dictate')));
      await tester.pumpAndSettle();
      expect(rig.voice.listens, 2);

      // Send presses the composer's Send.
      await tester.tap(find.byKey(const ValueKey('send')));
      await tester.pump();
      expect(rig.surface.js.last, 'window.claxHost?.submitDraft()');

      // The composer closing ends the peek and the listen.
      rig.surface.push(_Rig.noDraft);
      await tester.pumpAndSettle();
      expect(rig.voice.listening, isFalse);
      expect(find.text('Versions'), findsOneWidget);
    },
  );

  testWidgets('dictation keeps the text typed before it', (tester) async {
    final rig = await pumpScreen(
      tester,
      voice: FakeVoice(script: const ['blue'], hold: true),
    );
    rig.surface.push(rig.ready());
    await openPanel(tester);
    rig.surface.push(
      const StateMsg(
        commenting: true,
        draft: Draft(label: 'h1', text: 'make it', clip: DraftClip.ready),
        busy: false,
      ),
    );
    await tester.pumpAndSettle();
    expect(
      rig.surface.js.last,
      'window.claxHost?.setDraftText("make it blue")',
    );
  });

  testWidgets('without Dictate comments a pick waits for the mic button', (
    tester,
  ) async {
    final rig = await pumpScreen(
      tester,
      settings: AppSettings(dictateComments: false),
    );
    rig.surface.push(rig.ready());
    await openPanel(tester);
    rig.surface.push(_Rig.draft);
    await tester.pumpAndSettle();
    expect(rig.voice.listens, 0);
    await tester.tap(find.byKey(const ValueKey('dictate')));
    await tester.pumpAndSettle();
    expect(rig.voice.listens, 1);
  });

  testWidgets('a denied microphone disables the mic and says why', (
    tester,
  ) async {
    final rig = await pumpScreen(
      tester,
      voice: FakeVoice.denied('Microphone access is turned off'),
    );
    rig.surface.push(rig.ready());
    await openPanel(tester);
    rig.surface.push(_Rig.draft);
    await tester.pumpAndSettle();
    expect(rig.voice.listens, 0);
    expect(find.text('Microphone access is turned off'), findsOneWidget);
    final mic = tester.widget<IconButton>(
      find.byKey(const ValueKey('dictate')),
    );
    expect(mic.onPressed, isNull);
  });

  testWidgets(
    'Sent shows Working until the version lands, which stops the timing',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      rig.surface.push(_Rig.draft);
      await tester.pumpAndSettle();
      rig.surface.push(const Sent('thread-1'));
      rig.surface.push(_Rig.noDraft);
      await tester.pump();
      expect(find.text('Working…'), findsOneWidget);
      expect(rig.voice.listening, isFalse);

      rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
      rig.worker.add(WorkerLanded(rig.aid, 4));
      await tester.pump();
      expect(
        find.text('Working…'),
        findsOneWidget,
        reason: 'until the frame shows it',
      );

      rig.surface.push(const Landed(4));
      await tester.pump();
      expect(find.text('Working…'), findsNothing);
      expect(rig.timings.log, [
        'start $longpressLanded',
        'stop $longpressLanded',
      ]);
      expect(rig.timings.extras.single, {'artifact': rig.aid, 'version': 4});
    },
  );

  testWidgets('a reply without a version ends Working and drops the timing', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await openPanel(tester);
    rig.surface.push(_Rig.draft);
    rig.surface.push(const Sent('thread-1'));
    rig.surface.push(_Rig.noDraft);
    await tester.pump();
    rig.worker.add(WorkerReplied(rig.aid));
    await tester.pump();
    expect(find.text('Working…'), findsNothing);
    expect(find.text('Answered in the thread'), findsOneWidget);
    // A version landing later (a restore) is not this change's.
    rig.surface.push(const Landed(4));
    await tester.pump();
    expect(rig.timings.log, ['start $longpressLanded']);
  });

  testWidgets(
    'a failed change shows its reason; a cancelled one shows as stopped',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      rig.surface.push(const Sent('thread-1'));
      await tester.pump();
      rig.worker.add(WorkerFailed(rig.aid, 'The API key was refused'));
      await tester.pump();
      expect(find.text('Working…'), findsNothing);
      expect(find.byKey(const ValueKey('chip-failed')), findsOneWidget);
      expect(find.textContaining('The API key was refused'), findsOneWidget);

      rig.surface.push(const Sent('thread-2'));
      await tester.pump();
      expect(find.text('Working…'), findsOneWidget);
      rig.worker.add(WorkerCancelled(rig.aid));
      await tester.pump();
      expect(find.byKey(const ValueKey('chip-failed')), findsNothing);
      expect(find.byKey(const ValueKey('chip-stopped')), findsOneWidget);
      expect(rig.timings.log, isNot(contains('stop $longpressLanded')));
    },
  );

  testWidgets("another app's worker events are ignored", (tester) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    rig.surface.push(const Sent('thread-1'));
    await tester.pump();
    rig.worker.add(WorkerReplied('other'));
    rig.worker.add(WorkerFailed('other', 'nope'));
    await tester.pump();
    expect(find.text('Working…'), findsOneWidget);
  });

  testWidgets(
    'a change that lands while an older version is shown ends Working',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready(version: 2, latest: 3));
      rig.surface.push(const Sent('thread-1'));
      await tester.pump();
      rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
      rig.worker.add(WorkerLanded(rig.aid, 4));
      await tester.pump();
      expect(find.text('Working…'), findsNothing);
      expect(find.text('Version 4 is ready'), findsOneWidget);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Show'));
      await tester.pump();
      expect(rig.surface.js.last, 'window.claxHost?.loadVersion(null)');
    },
  );

  testWidgets('the Versions tab previews and restores versions', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await openPanel(tester);
    await tester.tap(find.text('Versions'));
    await tester.pumpAndSettle();
    // Newest first.
    final v3 = tester.getTopLeft(find.text('v3'));
    final v2 = tester.getTopLeft(find.text('v2'));
    final v1 = tester.getTopLeft(find.text('v1'));
    expect(v3.dy < v2.dy && v2.dy < v1.dy, isTrue);
    expect(find.text('Weekly total'), findsOneWidget);

    // A row opens its version page, whose Preview shows that version.
    await tester.tap(find.text('v2'));
    await tester.pumpAndSettle();
    expect(find.text('Version 2'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('preview')));
    await tester.pumpAndSettle();
    expect(find.text('Version 2'), findsNothing);
    expect(rig.surface.js.last, 'window.claxHost?.loadVersion(2)');
    await tester.tap(find.text('v3'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('preview')));
    await tester.pumpAndSettle();
    expect(rig.surface.js.last, 'window.claxHost?.loadVersion(null)');

    await tester.ensureVisible(find.byKey(const ValueKey('restore-1')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('restore-1')));
    await tester.pumpAndSettle();
    final restore = rig.api.calls.firstWhere((c) => c.name == 'restoreVersion');
    expect(restore.args, {'aid': rig.aid, 'n': 1, 'ifVersion': 3});
    expect(rig.surface.js.last, 'window.claxHost?.reloadLatest()');
    expect(rig.api.currentVersion(rig.aid), 4);
    expect(find.text('v4'), findsOneWidget, reason: 'the list is read again');
  });

  testWidgets('Undo restores the version before the latest', (tester) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await openPanel(tester);
    await tester.tap(find.text('Versions'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('undo')));
    await tester.pumpAndSettle();
    final restore = rig.api.calls.firstWhere((c) => c.name == 'restoreVersion');
    expect(restore.args, {'aid': rig.aid, 'n': 2, 'ifVersion': 3});
    expect(rig.surface.js.last, 'window.claxHost?.reloadLatest()');
  });

  testWidgets('a restore that loses a race says so and reads the list again', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await openPanel(tester);
    await tester.tap(find.text('Versions'));
    await tester.pumpAndSettle();
    rig.api.failNext(
      'restoreVersion',
      const ClaxApiError(
        status: 409,
        code: 'conflict',
        message: 'stale',
        body: {'current': 4},
      ),
    );
    await tester.ensureVisible(find.byKey(const ValueKey('restore-1')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('restore-1')));
    await tester.pumpAndSettle();
    expect(find.text('The app changed meanwhile. Try again.'), findsOneWidget);
    expect(rig.surface.js, isNot(contains('window.claxHost?.reloadLatest()')));
    expect(
      rig.api.callNames.where((n) => n == 'getArtifact').length,
      greaterThanOrEqualTo(2),
    );
  });

  testWidgets('the Threads tab lists the shell threads; a tap opens its page', (
    tester,
  ) async {
    final retries = <(String, String)>[];
    final rig = await pumpScreen(
      tester,
      onRetry: (aid, tid) async {
        retries.add((aid, tid));
        return true;
      },
    );
    final item = rig.api.sendComment(rig.aid, 'make the title blue');
    await rig.api.replyAsAgent(
      rig.aid,
      item.threadId,
      "Couldn't make that change: the network dropped",
    );
    await rig.api.publish(
      rig.aid,
      PublishRequest(
        ifVersion: 3,
        files: {'index.html': '<h1>Runs</h1>'},
        addresses: [item.threadId],
      ),
    );
    rig.surface.push(rig.ready());
    await openPanel(tester);
    expect(find.textContaining('Hold a finger'), findsOneWidget);
    rig.surface.push(
      ThreadsMsg([
        ThreadSummary(
          id: item.threadId,
          status: 'open',
          summary: 'make the title blue',
          sent: true,
          comments: 2,
          version: 3,
          label: 'the page title',
        ),
      ]),
    );
    await tester.pump();
    expect(find.textContaining('the page title'), findsOneWidget);
    await tester.tap(find.text('make the title blue'));
    await tester.pumpAndSettle();

    // The page: the shell's label, the whole conversation, and Retry.
    expect(find.byType(ThreadDetail), findsOneWidget);
    expect(find.text('The page title'), findsOneWidget);
    expect(find.text('make the title blue'), findsOneWidget);
    expect(find.text('Clax'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('thread-retry')));
    await tester.pump();
    expect(retries, [(rig.aid, item.threadId)]);

    // "Changed in v4" closes the page and previews version 4.
    rig.surface.js.clear();
    await tester.tap(find.text('Changed in v4'));
    await tester.pumpAndSettle();
    expect(find.byType(ThreadDetail), findsNothing);
    expect(rig.surface.js, contains('window.claxHost?.loadVersion(4)'));
  });

  testWidgets('without a retry hook the thread page offers no Retry', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    final item = rig.api.sendComment(rig.aid, 'make it blue');
    await rig.api.replyAsAgent(
      rig.aid,
      item.threadId,
      "Couldn't make that change: x",
    );
    rig.surface.push(rig.ready());
    await openPanel(tester);
    rig.surface.push(
      ThreadsMsg([
        ThreadSummary(
          id: item.threadId,
          status: 'open',
          summary: 'make it blue',
          sent: true,
          comments: 2,
          version: 3,
        ),
      ]),
    );
    await tester.pump();
    await tester.tap(find.text('make it blue'));
    await tester.pumpAndSettle();
    expect(find.text('Couldn’t make that change'), findsOneWidget);
    expect(find.byKey(const ValueKey('thread-retry')), findsNothing);
  });

  testWidgets('a shell notice shows as a snack bar', (tester) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    rig.surface.push(const Notice('Could not post the comment'));
    await tester.pump();
    expect(find.text('Could not post the comment'), findsOneWidget);
  });

  testWidgets(
    'a reloading page is covered again; one that gave up offers Reload',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.fault(const SurfaceFault('gone', reloading: true));
      await tester.pump();
      expect(find.byKey(const ValueKey('app-cover')), findsOneWidget);
      expect(find.text('Reload'), findsNothing);
      rig.surface.fault(const SurfaceFault('gone again', reloading: false));
      await tester.pump();
      expect(find.text('Reload'), findsOneWidget);
      await tester.tap(find.text('Reload'));
      await tester.pump();
      expect(rig.surface.reloads, 1);
      expect(find.text('Reload'), findsNothing);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(find.byKey(const ValueKey('app-cover')), findsNothing);
    },
  );

  testWidgets(
    'a reloaded page with the panel open turns comment mode on again',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      rig.surface.push(_Rig.noDraft);
      rig.surface.push(rig.ready());
      expect(rig.surface.js, ['window.claxHost?.enterCommentMode()']);
      rig.surface.push(
        const StateMsg(commenting: false, draft: null, busy: false),
      );
      rig.surface.push(rig.ready());
      expect(rig.surface.js, [
        'window.claxHost?.enterCommentMode()',
        'window.claxHost?.enterCommentMode()',
      ]);
    },
  );

  testWidgets('a failed send offers Retry, which sends the thread again', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    rig.surface.push(const SendFailed('thread-9'));
    await tester.pump();
    expect(find.text('Couldn’t send the comment'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('retry-send')));
    await tester.pump();
    expect(rig.surface.js.last, 'window.claxHost?.resend("thread-9")');
    expect(find.byKey(const ValueKey('retry-send')), findsNothing);
  });

  testWidgets('an open thread that was never sent can be sent from the panel', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await openPanel(tester);
    rig.surface.push(
      const ThreadsMsg([
        ThreadSummary(
          id: 't5',
          status: 'open',
          summary: 'bigger buttons',
          sent: false,
          comments: 1,
          version: 3,
        ),
      ]),
    );
    await tester.pump();
    expect(find.textContaining('not sent'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('resend-t5')));
    await tester.pump();
    expect(rig.surface.js.last, 'window.claxHost?.resend("t5")');
  });

  testWidgets(
    'a frame load while the composer is open leaves comment mode to the shell',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      // While its composer is open the shell reports comment mode off.
      rig.surface.push(
        const StateMsg(
          commenting: false,
          draft: Draft(label: 'h1', text: '', clip: DraftClip.ready),
          busy: false,
        ),
      );
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(rig.surface.js.where((j) => j.contains('CommentMode')).toList(), [
        'window.claxHost?.enterCommentMode()',
      ]);
      await tester.pumpAndSettle();
    },
  );

  testWidgets('a failure that comes before Send still ends Working', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await tester.pump();
    rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
    rig.worker.add(WorkerFailed(rig.aid, 'The API key was refused'));
    rig.surface.push(const Sent('thread-1'));
    await tester.pump();
    expect(find.text('Working…'), findsNothing);
    expect(find.byKey(const ValueKey('chip-failed')), findsOneWidget);
  });

  testWidgets('a page reloaded mid-change ends Working on the newer version', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    rig.surface.push(const Sent('thread-1'));
    rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
    rig.worker.add(WorkerLanded(rig.aid, 4));
    await tester.pump();
    expect(find.text('Working…'), findsOneWidget);
    rig.surface.fault(const SurfaceFault('gone', reloading: true));
    rig.surface.push(rig.ready(version: 4, latest: 4));
    await tester.pump();
    expect(find.text('Working…'), findsNothing);
    expect(rig.timings.log, [
      'start $longpressLanded',
      'stop $longpressLanded',
    ]);
    expect(rig.timings.extras.single['version'], 4);
  });

  testWidgets("a Restore's landing leaves another change Working and timed", (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('panel-handle')));
    await frames(tester);
    rig.surface.push(const Sent('thread-1'));
    rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
    await tester.tap(find.text('Versions'));
    await frames(tester);
    await tester.tap(find.byKey(const ValueKey('undo')));
    await frames(tester);
    rig.surface.push(const Landed(4));
    await tester.pump();
    expect(find.text('Working…'), findsOneWidget);
    expect(rig.timings.log, ['start $longpressLanded']);
  });

  testWidgets('Working can be dismissed, and comes back with new work', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    rig.surface.push(const Sent('thread-1'));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('chip-dismiss')));
    await tester.pump();
    expect(find.text('Working…'), findsNothing);
    rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
    await tester.pump();
    expect(find.text('Working…'), findsOneWidget);
  });

  testWidgets(
    'reopening the panel mid-draft leaves comment mode to the shell',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      rig.surface.push(
        const StateMsg(
          commenting: false,
          draft: Draft(label: 'the title', text: '', clip: DraftClip.ready),
          busy: false,
        ),
      );
      await tester.pumpAndSettle();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('app-panel')), findsNothing);
      await tester.tap(find.byKey(const ValueKey('panel-handle')));
      await tester.pumpAndSettle();
      expect(rig.surface.js.where((j) => j.contains('CommentMode')).toList(), [
        'window.claxHost?.enterCommentMode()',
      ], reason: 'only the first open, before the pick');
    },
  );

  testWidgets(
    'a composer closed while the panel is closed leaves comment mode off',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      rig.surface.push(
        const StateMsg(
          commenting: false,
          draft: Draft(label: '', text: '', clip: DraftClip.ready),
          busy: false,
        ),
      );
      await tester.pumpAndSettle();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      // The shell turns comment mode back on as the composer closes.
      rig.surface.push(_Rig.noDraft);
      await tester.pumpAndSettle();
      expect(rig.surface.js.last, 'window.claxHost?.exitCommentMode()');
      expect(find.byKey(const ValueKey('app-panel')), findsNothing);
    },
  );

  testWidgets(
    'the page is visible while the app is resumed and the screen on top',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(rig.surface.visibility, [true]);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      await tester.pump();
      expect(rig.surface.visibility, [true], reason: 'inactive is still shown');
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      await tester.pump();
      expect(rig.surface.visibility, [true, false]);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      expect(rig.surface.visibility, [true, false, true]);

      // Another screen over it hides it; coming back shows it.
      final nav = tester.state<NavigatorState>(find.byType(Navigator));
      unawaited(
        nav.push(
          MaterialPageRoute<void>(builder: (_) => const Text('Settings')),
        ),
      );
      await tester.pumpAndSettle();
      expect(rig.surface.visibility, [true, false, true, false]);
      nav.pop();
      await tester.pumpAndSettle();
      expect(rig.surface.visibility, [true, false, true, false, true]);
    },
  );

  testWidgets('the screen disposes its surface', (tester) async {
    final rig = await pumpScreen(tester);
    await tester.pumpWidget(const SizedBox());
    expect(rig.surface.disposed, isTrue);
  });

  group('the comment button', () {
    final button = find.byKey(const ValueKey('comment-button'));
    final bar = find.byKey(const ValueKey('comment-bar'));
    final panel = find.byKey(const ValueKey('app-panel'));
    List<String> modes(_Rig rig) =>
        rig.surface.js.where((j) => j.contains('CommentMode')).toList();
    const enter = 'window.claxHost?.enterCommentMode()';
    const exit = 'window.claxHost?.exitCommentMode()';

    testWidgets('shows once the page is ready, bottom right', (tester) async {
      final rig = await pumpScreen(tester);
      expect(button, findsNothing, reason: 'covered until ready');
      rig.surface.push(rig.ready());
      await tester.pump();
      final r = tester.getRect(button);
      expect(r.right, 360 - CommentButton.margin);
      expect(r.bottom, 780 - CommentButton.defaultDy);
    });

    testWidgets(
      'tap turns comment mode on with the bar under the page; close turns it off',
      (tester) async {
        final rig = await pumpScreen(tester);
        rig.surface.push(rig.ready());
        await tester.pump();
        await tester.tap(button);
        await tester.pumpAndSettle();
        expect(modes(rig), [enter]);
        expect(bar, findsOneWidget);
        expect(button, findsNothing, reason: 'the bar takes over');
        expect(panel, findsNothing);
        // The page sits above the bar, so nothing on it is covered.
        final page = tester.getRect(find.byKey(const ValueKey('fake-surface')));
        final b = tester.getRect(bar);
        expect(page.bottom, closeTo(b.top, 0.01));
        expect(b.bottom, 780);

        // The shell confirms; nothing is turned off.
        rig.surface.push(_Rig.noDraft);
        await tester.pump();
        expect(modes(rig), [enter]);

        await tester.tap(find.byKey(const ValueKey('comment-bar-close')));
        await tester.pumpAndSettle();
        expect(modes(rig), [enter, exit]);
        expect(bar, findsNothing);
        expect(button, findsOneWidget);
      },
    );

    testWidgets('long-press opens the threads panel, as the swipe does', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await tester.longPress(button);
      await tester.pumpAndSettle();
      expect(panel, findsOneWidget);
      expect(modes(rig), [enter]);
      expect(button, findsNothing, reason: 'hidden under a narrow panel');
      expect(find.textContaining('Hold a finger'), findsOneWidget);
    });

    testWidgets(
      'Whole app composes on the whole app and dictates; after it the bar is back',
      (tester) async {
        final rig = await pumpScreen(tester);
        rig.surface.push(rig.ready());
        await tester.pump();
        await tester.tap(button);
        await tester.pumpAndSettle();
        await tester.tap(find.byKey(const ValueKey('comment-whole-app')));
        await tester.pump();
        expect(rig.surface.js.last, 'window.claxHost?.composeArtifact()');

        rig.surface.push(
          const StateMsg(
            commenting: false,
            draft: Draft(
              label: 'The whole app',
              text: '',
              clip: DraftClip.none,
            ),
            busy: false,
          ),
        );
        await tester.pumpAndSettle();
        expect(rig.voice.listens, 1, reason: 'Dictate comments is on');
        expect(find.text('The whole app'), findsOneWidget);
        expect(button, findsNothing);
        expect(bar, findsNothing, reason: "the peek's controls take over");

        await tester.tap(find.byKey(const ValueKey('send')));
        await tester.pump();
        rig.surface.push(const Sent('t9'));
        rig.surface.push(_Rig.noDraft);
        // Working spins on, so the frames are counted out.
        await frames(tester);
        expect(panel, findsNothing, reason: 'the pick opened it');
        expect(bar, findsOneWidget);
        expect(modes(rig), [enter], reason: 'comment mode stays on');
      },
    );

    testWidgets('without Dictate comments Whole app waits for the mic', (
      tester,
    ) async {
      final rig = await pumpScreen(
        tester,
        settings: AppSettings(dictateComments: false),
      );
      rig.surface.push(rig.ready());
      await tester.pump();
      await tester.tap(button);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('comment-whole-app')));
      rig.surface.push(
        const StateMsg(
          commenting: false,
          draft: Draft(label: 'The whole app', text: '', clip: DraftClip.none),
          busy: false,
        ),
      );
      await tester.pumpAndSettle();
      expect(rig.voice.listens, 0);
    });

    testWidgets(
      'Threads opens the panel; closing it goes back to the bar, still commenting',
      (tester) async {
        final rig = await pumpScreen(tester);
        rig.surface.push(rig.ready());
        rig.surface.push(
          const ThreadsMsg([
            ThreadSummary(
              id: 't1',
              status: 'open',
              summary: 'bigger',
              sent: true,
              comments: 1,
              version: 3,
            ),
            ThreadSummary(
              id: 't2',
              status: 'resolved',
              summary: 'bluer',
              sent: true,
              comments: 2,
              version: 2,
            ),
          ]),
        );
        await tester.pump();
        expect(find.text('1'), findsOneWidget, reason: 'one open thread');
        await tester.tap(button);
        await tester.pumpAndSettle();
        expect(find.text('Threads (1)'), findsOneWidget);
        await tester.tap(find.byKey(const ValueKey('comment-threads')));
        await tester.pumpAndSettle();
        expect(panel, findsOneWidget);
        expect(bar, findsNothing);
        await tester.binding.handlePopRoute();
        await tester.pumpAndSettle();
        expect(panel, findsNothing);
        expect(bar, findsOneWidget);
        expect(modes(rig), [enter]);
      },
    );

    testWidgets('a reloaded shell gets comment mode back while the bar is up', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await tester.tap(button);
      await tester.pumpAndSettle();
      rig.surface.push(
        const StateMsg(commenting: false, draft: null, busy: false),
      );
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(modes(rig), [enter, enter]);
    });

    testWidgets(
      'on a wide screen it stays beside the open panel; its tap closes both',
      (tester) async {
        final rig = await pumpScreen(tester, size: const Size(1200, 800));
        rig.surface.push(rig.ready());
        await tester.pump();
        await tester.longPress(button);
        await tester.pumpAndSettle();
        expect(panel, findsOneWidget);
        expect(button, findsOneWidget);
        expect(
          tester.getRect(button).right,
          lessThanOrEqualTo(tester.getRect(panel).left),
        );
        expect(
          find.descendant(of: button, matching: find.byIcon(Icons.close)),
          findsOneWidget,
        );
        await tester.tap(button);
        await tester.pumpAndSettle();
        expect(panel, findsNothing);
        expect(bar, findsNothing);
        expect(modes(rig), [enter, exit]);
      },
    );

    testWidgets('a drag moves it and reports its height', (tester) async {
      final moves = <double>[];
      final rig = await pumpScreen(
        tester,
        buttonDy: 200,
        onButtonMoved: moves.add,
      );
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(tester.getRect(button).bottom, 780 - 200);
      await tester.dragFrom(tester.getCenter(button), const Offset(-300, -100));
      await tester.pumpAndSettle();
      expect(moves.single, closeTo(300, 1));
      expect(tester.getRect(button).left, CommentButton.margin);
      expect(modes(rig), isEmpty);
      expect(rig.surface.attests, isEmpty, reason: 'not a touch on the page');
    });

    testWidgets('back with the bar up ends comment mode and keeps the screen', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await tester.tap(button);
      await tester.pumpAndSettle();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byType(AppScreen), findsOneWidget);
      expect(bar, findsNothing);
      expect(button, findsOneWidget);
      expect(modes(rig), [enter, exit]);
    });

    testWidgets('back closes the panel opened from the bar, then the bar', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await tester.tap(button);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('comment-threads')));
      await tester.pumpAndSettle();
      expect(panel, findsOneWidget);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(panel, findsNothing);
      expect(bar, findsOneWidget);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(bar, findsNothing);
      expect(find.byType(AppScreen), findsOneWidget);
      expect(modes(rig), [enter, exit]);
    });

    testWidgets(
      'Escape and the Comments command end comment mode from the bar',
      (tester) async {
        final cmd = CommentsCommand();
        addTearDown(cmd.dispose);
        final rig = await pumpScreen(
          tester,
          size: const Size(1200, 800),
          comments: cmd,
        );
        rig.surface.push(rig.ready());
        await tester.pump();
        await tester.tap(button);
        await tester.pumpAndSettle();
        expect(bar, findsOneWidget);
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await tester.pumpAndSettle();
        expect(bar, findsNothing);
        expect(modes(rig), [enter, exit]);

        await tester.tap(button);
        await tester.pumpAndSettle();
        cmd.toggle!();
        await tester.pumpAndSettle();
        expect(bar, findsNothing);
        expect(panel, findsNothing, reason: 'it ends the mode, opens nothing');
        expect(modes(rig), [enter, exit, enter, exit]);
        // With nothing up, the command opens the panel as before.
        cmd.toggle!();
        await tester.pumpAndSettle();
        expect(panel, findsOneWidget);
      },
    );

    testWidgets(
      'a pick from the bar puts the peek in its place: the page keeps its height',
      (tester) async {
        final rig = await pumpScreen(tester);
        rig.surface.push(rig.ready());
        await tester.pump();
        await tester.tap(button);
        await tester.pumpAndSettle();
        final page = find.byKey(const ValueKey('fake-surface'));
        final withBar = tester.getRect(page);
        rig.surface.push(_Rig.draft);
        await tester.pumpAndSettle();
        expect(panel, findsOneWidget);
        expect(tester.getRect(page), withBar);
        rig.surface.push(_Rig.noDraft);
        await tester.pumpAndSettle();
        expect(bar, findsOneWidget);
        expect(tester.getRect(page), withBar);
      },
    );

    testWidgets('the two-finger swipe still opens the panel', (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await twoFingerSwipe(tester, -120);
      expect(panel, findsOneWidget);
      expect(button, findsNothing);
    });
  });

  testWidgets('an OS-cancelled touch on the page is attested as cancel', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await tester.pump();
    final g = await tester.startGesture(const Offset(80, 300), pointer: 5);
    await g.cancel();
    await tester.pump();
    final a = rig.surface.attests;
    expect(
      [for (final p in a) p.phase],
      [PointerPhase.down, PointerPhase.cancel],
    );
    expect(a.every((p) => p.pointer == 5), isTrue);
  });

  testWidgets(
    'on macOS, App > Comments (Cmd-K) toggles the panel and Escape closes it',
    (tester) async {
      final menus = <Object?>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.menu,
        (call) async {
          if (call.method == 'Menu.setMenus') menus.add(call.arguments);
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.menu,
          null,
        ),
      );
      final rig = await pumpScreen(tester, size: const Size(1200, 800));
      rig.surface.push(rig.ready());
      await tester.pump();

      final item = _findItem(menus.last, 'Comments');
      expect(item, isNotNull);
      expect(item!['shortcutTrigger'], LogicalKeyboardKey.keyK.keyId);
      // The menus are sent again on each rebuild, with new IDs.
      Future<void> select() async {
        final id = _findItem(menus.last, 'Comments')!['id'];
        await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
          'flutter/menu',
          SystemChannels.menu.codec.encodeMethodCall(
            MethodCall('Menu.selectedCallback', id),
          ),
          (_) {},
        );
        await tester.pumpAndSettle();
      }

      await select();
      expect(find.byKey(const ValueKey('app-panel')), findsOneWidget);
      await select();
      expect(find.byKey(const ValueKey('app-panel')), findsNothing);
      await select();
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('app-panel')), findsNothing);
      expect(rig.surface.js, [
        'window.claxHost?.enterCommentMode()',
        'window.claxHost?.exitCommentMode()',
        'window.claxHost?.enterCommentMode()',
        'window.claxHost?.exitCommentMode()',
      ]);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );
}

Map<Object?, Object?>? _findItem(Object? tree, String label) {
  if (tree is Map) {
    if (tree['label'] == label) return tree;
    for (final v in tree.values) {
      final f = _findItem(v, label);
      if (f != null) return f;
    }
  } else if (tree is List) {
    for (final v in tree) {
      final f = _findItem(v, label);
      if (f != null) return f;
    }
  }
  return null;
}
