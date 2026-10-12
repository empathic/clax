import 'dart:async';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

const _page = {'index.html': '<!doctype html><title>x</title>'};

/// A fake whose stamps increase, so later artifacts are newer.
FakeClaxApi _api() {
  var t = 0;
  return FakeClaxApi(
    clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
  );
}

void main() {
  late List<String> opened;

  setUp(() => opened = []);

  Future<void> pumpLauncher(
    WidgetTester tester,
    ClaxApi api, {
    Future<String> Function(String)? make,
    MakeApp? makeFull,
    FakeVoice? voice,
    VoidCallback? openSettings,
    bool needsKey = false,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Launcher(
          api: api,
          open: opened.add,
          make:
              makeFull ??
              (r, cancelled, stage) =>
                  (make ?? (_) async => throw StateError('unused'))(r),
          voice: voice ?? FakeVoice.unsupported('No speech here'),
          openSettings: openSettings,
          needsKey: needsKey,
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('shows the apps newest first', (tester) async {
    final api = _api();
    api.seedArtifact('Old runs', _page);
    api.seedArtifact('Groceries', _page);
    await pumpLauncher(tester, api);
    final older = tester.getTopLeft(find.text('Old runs'));
    final newer = tester.getTopLeft(find.text('Groceries'));
    expect(
      newer.dy < older.dy || (newer.dy == older.dy && newer.dx < older.dx),
      isTrue,
      reason: 'the newest app comes first',
    );
  });

  testWidgets('an empty launcher invites the first app', (tester) async {
    await pumpLauncher(tester, _api());
    expect(find.text('Say what you want, and Clax makes it.'), findsOneWidget);
    expect(find.text('New app'), findsOneWidget);
  });

  testWidgets('a version event for a new artifact adds its tile', (
    tester,
  ) async {
    final api = _api();
    api.seedArtifact('Old runs', _page);
    await pumpLauncher(tester, api);
    final aid = api.seedArtifact('Fresh app', _page);
    expect(find.text('Fresh app'), findsNothing);
    api.emit(ClaxEvent(type: 'version', data: {'artifact_id': aid, 'n': 1}));
    await tester.pumpAndSettle();
    expect(find.text('Fresh app'), findsOneWidget);
    expect(
      api.calls.where((c) => c.name == 'events').single.args['types'],
      containsAll(['version', 'artifact_deleted']),
    );
  });

  testWidgets('an artifact_deleted event removes its tile', (tester) async {
    final api = _api();
    final aid = api.seedArtifact('Doomed', _page);
    await pumpLauncher(tester, api);
    expect(find.text('Doomed'), findsOneWidget);
    api.emit(ClaxEvent(type: 'artifact_deleted', data: {'artifact_id': aid}));
    await tester.pumpAndSettle();
    expect(find.text('Doomed'), findsNothing);
  });

  testWidgets('tapping a tile opens its app', (tester) async {
    final api = _api();
    final aid = api.seedArtifact('Runs', _page);
    await pumpLauncher(tester, api);
    await tester.tap(find.text('Runs'));
    expect(opened, [aid]);
  });

  testWidgets('New, typing and submit makes the app and opens it', (
    tester,
  ) async {
    final api = _api();
    final asked = <String>[];
    await pumpLauncher(
      tester,
      api,
      make: (r) async {
        asked.add(r);
        return 'made123';
      },
    );
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    expect(find.text('Say or type what you want'), findsOneWidget);
    await tester.enterText(find.byType(TextField), 'a tracker for my runs');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pumpAndSettle();
    expect(asked, ['a tracker for my runs']);
    expect(opened, ['made123']);
    expect(find.textContaining('Making'), findsNothing);
  });

  testWidgets('making shows the request, the seconds, and Cancel', (
    tester,
  ) async {
    final pending = Completer<String>();
    await pumpLauncher(tester, _api(), make: (_) => pending.future);
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byType(TextField),
      'a tracker for my runs with a weekly total',
    );
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(
      find.text('Making “a tracker for my runs with a weekly total”…'),
      findsOneWidget,
    );
    expect(find.text('Starting the agent'), findsOneWidget);
    expect(find.text('0 s'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
    expect(find.text('3 s'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(find.textContaining('Making'), findsNothing);
    // The abandoned run finishing later opens nothing.
    pending.complete('late');
    await tester.pumpAndSettle();
    expect(opened, isEmpty);
    // The request is kept for the next New.
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    expect(
      find.text('a tracker for my runs with a weekly total'),
      findsOneWidget,
    );
  });

  testWidgets('a failed make shows the reason and Retry, keeping the text', (
    tester,
  ) async {
    var attempts = 0;
    await pumpLauncher(
      tester,
      _api(),
      make: (_) async {
        if (++attempts == 1) throw const AgentError('The model is busy');
        return 'second';
      },
    );
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'a habit tracker');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pumpAndSettle();
    expect(find.text('The model is busy'), findsOneWidget);
    expect(find.text('a habit tracker'), findsOneWidget);
    expect(opened, isEmpty);
    await tester.tap(find.text('Retry'));
    await tester.pumpAndSettle();
    expect(attempts, 2);
    expect(opened, ['second']);
  });

  testWidgets('a failure naming Settings links there', (tester) async {
    var settings = 0;
    await pumpLauncher(
      tester,
      _api(),
      make: (_) async =>
          throw const AgentError('Add your Claude API key in Settings'),
      openSettings: () => settings++,
    );
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'a timer');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pumpAndSettle();
    expect(find.text('Add your Claude API key in Settings'), findsOneWidget);
    await tester.tap(find.text('Open Settings'));
    await tester.pumpAndSettle();
    expect(settings, 1);
  });

  testWidgets('the settings button shows only when it has somewhere to go', (
    tester,
  ) async {
    await pumpLauncher(tester, _api());
    expect(find.byTooltip('Settings'), findsNothing);
    var settings = 0;
    await pumpLauncher(tester, _api(), openSettings: () => settings++);
    await tester.tap(find.byTooltip('Settings'));
    expect(settings, 1);
  });

  testWidgets('the launcher follows the dark theme', (tester) async {
    final api = _api();
    api.seedArtifact('Runs', _page);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light),
        darkTheme: claxTheme(Brightness.dark),
        themeMode: ThemeMode.dark,
        home: Launcher(
          api: api,
          open: opened.add,
          make: (_, _, _) async => 'x',
          voice: FakeVoice.unsupported('No speech here'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final scaffold = tester.widget<Scaffold>(find.byType(Scaffold));
    final bg =
        scaffold.backgroundColor ??
        Theme.of(tester.element(find.byType(Scaffold))).scaffoldBackgroundColor;
    expect(bg.computeLuminance(), lessThan(0.1));
    expect(
      Theme.of(tester.element(find.text('Runs'))).brightness,
      Brightness.dark,
    );
  });

  Future<void> submit(WidgetTester tester, String request) async {
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), request);
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
  }

  /// A make that honours its cancel signal the way AgentLoop.newApp does:
  /// it creates the artifact only when [proceed] completes first.
  MakeApp honouring(
    FakeClaxApi api,
    Completer<void> proceed,
    List<Future<void>> signals,
  ) => (request, cancelled, stage) async {
    signals.add(cancelled);
    var stopped = false;
    await Future.any<void>([
      proceed.future,
      cancelled.then((_) {
        stopped = true;
      }),
    ]);
    if (stopped) throw const AgentError.cancelled();
    final aid = api.seedArtifact(request, _page);
    api.emit(ClaxEvent(type: 'version', data: {'artifact_id': aid, 'n': 1}));
    return aid;
  };

  testWidgets('Cancel stops the run: no app is created and nothing opens', (
    tester,
  ) async {
    final api = _api();
    final proceed = Completer<void>();
    final signals = <Future<void>>[];
    await pumpLauncher(tester, api, makeFull: honouring(api, proceed, signals));
    await submit(tester, 'a stopwatch');
    var fired = false;
    signals.single.then((_) => fired = true);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(fired, isTrue);
    proceed.complete();
    await tester.pumpAndSettle();
    expect(opened, isEmpty);
    expect(find.text('a stopwatch'), findsNothing);
    expect(await api.listArtifacts(), isEmpty);
  });

  testWidgets('system back on the making screen stops the run too', (
    tester,
  ) async {
    final api = _api();
    final signals = <Future<void>>[];
    await pumpLauncher(
      tester,
      api,
      makeFull: honouring(api, Completer<void>(), signals),
    );
    await submit(tester, 'a stopwatch');
    var fired = false;
    signals.single.then((_) => fired = true);
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(find.textContaining('Making'), findsNothing);
    expect(fired, isTrue);
  });

  testWidgets('Retry runs with a fresh signal; Edit stops the old one', (
    tester,
  ) async {
    final signals = <Future<void>>[];
    await pumpLauncher(
      tester,
      _api(),
      makeFull: (r, cancelled, stage) async {
        signals.add(cancelled);
        throw const AgentError('The model is busy');
      },
    );
    await submit(tester, 'a timer');
    await tester.pumpAndSettle();
    await tester.tap(find.text('Retry'));
    await tester.pumpAndSettle();
    expect(signals, hasLength(2));
    final fired = <int>[];
    for (var i = 0; i < 2; i++) {
      signals[i].then((_) => fired.add(i));
    }
    await tester.pump();
    expect(fired, [0], reason: 'a retry ends the failed attempt only');
    await tester.tap(find.text('Edit'));
    await tester.pumpAndSettle();
    expect(fired, [0, 1]);
    // Edit reopens the sheet with the request.
    expect(find.text('a timer'), findsOneWidget);
  });

  testWidgets('the making screen shows the stage make reports', (tester) async {
    final pending = Completer<String>();
    await pumpLauncher(
      tester,
      _api(),
      makeFull: (r, cancelled, stage) {
        stage(MakeStage.writing);
        return pending.future;
      },
    );
    await submit(tester, 'a timer');
    await tester.pump();
    expect(find.text('Writing your app'), findsOneWidget);
    expect(find.text('Usually under a minute'), findsNothing);
    await tester.pump(const Duration(seconds: 21));
    expect(find.text('Usually under a minute'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
  });

  testWidgets('resync and a fresh ready reload the list; a resumed ready does '
      'not', (tester) async {
    final api = _api();
    await pumpLauncher(tester, api);
    int loads() => api.callNames.where((n) => n == 'listArtifacts').length;
    final before = loads();
    api.seedArtifact('Made while away', _page);
    api.emit(const ClaxEvent(type: 'ready', data: {'resumed': true}));
    await tester.pumpAndSettle();
    expect(loads(), before);
    expect(find.text('Made while away'), findsNothing);
    api.emit(const ClaxEvent(type: 'resync', data: {}));
    await tester.pumpAndSettle();
    expect(find.text('Made while away'), findsOneWidget);
    api.seedArtifact('Made after a restart', _page);
    api.emit(const ClaxEvent(type: 'ready', data: {'resumed': false}));
    await tester.pumpAndSettle();
    expect(find.text('Made after a restart'), findsOneWidget);
  });

  testWidgets('without a key the launcher says so and links to Settings', (
    tester,
  ) async {
    var settings = 0;
    await pumpLauncher(
      tester,
      _api(),
      needsKey: true,
      openSettings: () => settings++,
    );
    expect(
      find.text('Add your Claude API key to start making apps'),
      findsOneWidget,
    );
    await tester.tap(find.text('Add key'));
    expect(settings, 1);
    await pumpLauncher(tester, _api(), openSettings: () {});
    expect(find.text('Add key'), findsNothing);
  });

  test('the making title ellipsizes a long request at a word boundary', () {
    const long =
        'a tracker for my runs with a weekly total and a monthly chart of pace';
    final t = makingTitle(long);
    expect(t, startsWith('Making “'));
    expect(t, endsWith('…”'));
    final quoted = t.substring('Making “'.length, t.length - 2);
    expect(quoted.length, lessThanOrEqualTo(48));
    expect(long.startsWith(quoted), isTrue);
    expect(long[quoted.length], ' ', reason: 'cut at a word boundary');
    expect(makingTitle('a timer'), 'Making “a timer”…');
    expect(t, 'Making “a tracker for my runs with a weekly total…”');
  });

  group('tiles', () {
    ArtifactSummary app(String id, String title, [String? icon]) =>
        ArtifactSummary(
          id: id,
          title: title,
          icon: icon,
          currentVersion: 1,
          updatedAt: '2026-10-12T00:00:00Z',
        );

    test('an emoji icon is shown as itself', () {
      expect(tileFace(app('a', 'Runs', '🏃')), const EmojiFace('🏃'));
    });

    test('a known icon word maps to its Material icon', () {
      expect(
        tileFace(app('a', 'Runs', 'chart')),
        const IconFace(Icons.insert_chart_rounded),
      );
    });

    test('otherwise a monogram of the title', () {
      expect(tileFace(app('a', 'Run log')), const MonogramFace('RL'));
      expect(tileFace(app('a', 'water')), const MonogramFace('W'));
      expect(
        tileFace(app('a', 'my reading list', 'zzz')),
        const MonogramFace('MR'),
      );
      expect(tileFace(app('a', '  ')), const MonogramFace('?'));
    });

    test('tile colours come from the ID', () {
      // Sequential IDs, as the daemon hands out, still spread over hues.
      final hues = [
        for (var i = 1; i <= 6; i++)
          HSLColor.fromColor(tileSeed('00000000000$i')).hue,
      ];
      final buckets = {for (final h in hues) h ~/ 60};
      expect(buckets.length, greaterThanOrEqualTo(4), reason: '$hues');
      expect(tileSeed('a1'), tileSeed('a1'));
    });

    testWidgets('tiles show monograms', (tester) async {
      final api = _api();
      api.seedArtifact('Run log', _page);
      api.seedArtifact('Groceries', _page);
      await pumpLauncher(tester, api);
      expect(find.text('RL'), findsOneWidget);
      expect(find.text('G'), findsOneWidget);
    });
  });
}
