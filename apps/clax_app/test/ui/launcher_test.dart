import 'dart:async';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/inbox_screen.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
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
    expect(api.callNames, isNot(contains('events')));
    final names = api.callNames;
    expect(
      api.calls.firstWhere((c) => c.name == 'streamSubscribe').args['topics'],
      containsAll(['gallery', 'inbox']),
    );
    expect(
      names.indexOf('streamSubscribe'),
      lessThan(names.indexOf('listArtifacts')),
      reason: 'subscribed before the list is fetched',
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

  testWidgets('with handOver the made app goes to it, not to open, and the '
      'making screen records on the timings', (tester) async {
    final api = _api();
    final handed = <String>[];
    final timings = _NoTimings();
    final made = Completer<String>();
    await tester.pumpWidget(
      MaterialApp(
        home: Launcher(
          api: api,
          open: opened.add,
          make: (r, cancelled, stage) => made.future,
          voice: FakeVoice.unsupported('No speech here'),
          timings: timings,
          handOver: (context, aid, frame) {
            handed.add(aid);
            Navigator.of(context).pushReplacement(
              MaterialPageRoute<void>(builder: (_) => const Text('the app')),
              result: Made(aid),
            );
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'a timer');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pump(const Duration(seconds: 1));
    await tester.pump(const Duration(seconds: 1));
    expect(
      tester.widget<MakingScreen>(find.byType(MakingScreen)).timings,
      same(timings),
    );
    made.complete('made123');
    await tester.pumpAndSettle();
    expect(handed, ['made123']);
    expect(opened, isEmpty);
    expect(find.text('the app'), findsOneWidget);
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

  /// A few frames: the making tiles' rings never settle.
  Future<void> frames(WidgetTester tester) async {
    for (var i = 0; i < 10; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
  }

  testWidgets('back leaves the make running as a tile, which shows it again; '
      'it lands as its app', (tester) async {
    final api = _api();
    final proceed = Completer<void>();
    final signals = <Future<void>>[];
    await pumpLauncher(tester, api, makeFull: honouring(api, proceed, signals));
    await submit(tester, 'a stopwatch');
    var fired = false;
    unawaited(signals.single.then((_) => fired = true));
    await tester.binding.handlePopRoute();
    await frames(tester);
    expect(find.text('Making “a stopwatch”…'), findsNothing);
    expect(fired, isFalse, reason: 'leaving never cancels');
    expect(find.byKey(const ValueKey('making-tile')), findsOneWidget);
    expect(find.byKey(const ValueKey('tile-making')), findsOneWidget);

    await tester.tap(find.text('a stopwatch'));
    await frames(tester);
    expect(find.text('Making “a stopwatch”…'), findsOneWidget);
    expect(signals, hasLength(1), reason: 'it attaches to the running make');

    await tester.tap(find.byKey(const ValueKey('build-back')));
    await frames(tester);
    proceed.complete();
    await frames(tester);
    expect(find.byKey(const ValueKey('making-tile')), findsNothing);
    expect(find.text('a stopwatch'), findsOneWidget, reason: 'its app tile');
    expect(opened, isEmpty, reason: 'nothing opens by itself');
  });

  testWidgets('two makes show two tiles; Cancel in one build view stops '
      'only that one', (tester) async {
    final api = _api();
    final signals = <Future<void>>[];
    await pumpLauncher(
      tester,
      api,
      makeFull: honouring(api, Completer<void>(), signals),
    );
    await submit(tester, 'a stopwatch');
    await tester.tap(find.byKey(const ValueKey('build-back')));
    await frames(tester);
    await tester.tap(find.text('New app'));
    await frames(tester);
    await tester.enterText(find.byType(TextField), 'a habit tracker');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await frames(tester);
    await tester.tap(find.byKey(const ValueKey('build-back')));
    await frames(tester);
    expect(find.byKey(const ValueKey('making-tile')), findsNWidgets(2));

    final fired = <int>[];
    for (var i = 0; i < 2; i++) {
      unawaited(signals[i].then((_) => fired.add(i)));
    }
    await tester.tap(find.text('a stopwatch'));
    await frames(tester);
    await tester.tap(find.text('Cancel'));
    await frames(tester);
    expect(fired, [0]);
    expect(find.byKey(const ValueKey('making-tile')), findsOneWidget);
    expect(find.text('a habit tracker'), findsOneWidget);
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

  testWidgets("gallery's resync reloads the list; inbox's reloads the count", (
    tester,
  ) async {
    final api = _api();
    await pumpLauncher(tester, api);
    int loads() => api.callNames.where((n) => n == 'listArtifacts').length;
    int counts() => api.callNames.where((n) => n == 'inboxSummary').length;
    final (lists, sums) = (loads(), counts());
    api.seedArtifact('Made while away', _page);
    api.emitStream(
      const StreamEvent(
        topic: 'inbox',
        type: 'resync',
        data: {'topic': 'inbox', 'reason': 'reconnect'},
      ),
    );
    await tester.pumpAndSettle();
    expect((loads(), counts()), (lists, sums + 1));
    expect(find.text('Made while away'), findsNothing);
    api.emitStream(
      const StreamEvent(
        topic: 'gallery',
        type: 'resync',
        data: {'topic': 'gallery', 'reason': 'reconnect'},
      ),
    );
    await tester.pumpAndSettle();
    expect(loads(), lists + 1);
    expect(find.text('Made while away'), findsOneWidget);
  });

  testWidgets('a shared stream is used for the launcher and its inbox, and '
      'left open', (tester) async {
    final api = _api();
    final shared = api.stream();
    final aid = api.seedArtifact('Runs', _page);
    await tester.pumpWidget(
      MaterialApp(
        home: Launcher(
          api: api,
          open: opened.add,
          make: (r, c, s) async => throw StateError('unused'),
          voice: FakeVoice.unsupported('No speech here'),
          stream: shared,
        ),
      ),
    );
    await tester.pumpAndSettle();
    api.emit(ClaxEvent(type: 'artifact_deleted', data: {'artifact_id': aid}));
    await tester.pumpAndSettle();
    expect(find.text('Runs'), findsNothing);
    await tester.tap(find.byTooltip('Inbox'));
    await tester.pumpAndSettle();
    expect(find.byType(InboxScreen), findsOneWidget);
    expect(
      tester.widget<InboxScreen>(find.byType(InboxScreen)).stream,
      same(shared),
    );
    await tester.pageBack();
    await tester.pumpAndSettle();
    await tester.pumpWidget(const SizedBox());
    await tester.pumpAndSettle();
    // Neither closed it: it still subscribes.
    await shared.subscribe(['gallery']);
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

  group('tile layout at 412 dp', () {
    for (final scale in [1.0, 1.3]) {
      testWidgets('two-line titles fit at text scale $scale', (tester) async {
        tester.view.physicalSize = const Size(412, 892);
        tester.view.devicePixelRatio = 1;
        addTearDown(tester.view.reset);
        final api = _api();
        for (final t in ['Mood journal', 'Weekly run tracker', 'Water']) {
          api.seedArtifact(t, _page);
        }
        await tester.pumpWidget(
          MaterialApp(
            builder: (context, child) => MediaQuery(
              data: MediaQuery.of(context)
                  .copyWith(textScaler: TextScaler.linear(scale)),
              child: child!,
            ),
            home: Launcher(
              api: api,
              open: opened.add,
              make: (_, _, _) async => 'x',
              voice: FakeVoice.unsupported('No speech here'),
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
        final title = tester.renderObject<RenderParagraph>(
          find.text('Weekly run tracker'),
        );
        expect(title.size.height, greaterThan(title.text.style!.fontSize!));
      });
    }

    testWidgets('a very long title ellipsizes on two lines', (tester) async {
      tester.view.physicalSize = const Size(412, 892);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final api = _api();
      const long =
          'A very long title for an app that tracks every single thing I do';
      api.seedArtifact(long, _page);
      await pumpLauncher(tester, api);
      expect(tester.takeException(), isNull);
      final p = tester.renderObject<RenderParagraph>(find.text(long));
      expect(p.maxLines, 2);
      expect(p.didExceedMaxLines, isTrue);
    });
  });

  group('inbox', () {
    late List<(String, String?, int?)> targets;
    late List<String> questionsOpened;
    late List<Duration> delays;

    setUp(() {
      delays = [];
      targets = [];
      questionsOpened = [];
    });

    Future<void> pumpWithInbox(
      WidgetTester tester,
      FakeClaxApi api, {
      bool targetHook = true,
      bool questionHook = false,
    }) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Launcher(
            api: api,
            open: opened.add,
            make: (_, _, _) async => throw StateError('unused'),
            voice: FakeVoice.unsupported('No speech here'),
            onOpenTarget: targetHook
                ? (aid, {thread, version}) =>
                      targets.add((aid, thread, version))
                : null,
            onOpenQuestion: questionHook ? questionsOpened.add : null,
            retryDelay: (d) async => delays.add(d),
          ),
        ),
      );
      await tester.pumpAndSettle();
    }

    Badge badge(WidgetTester tester) => tester.widget<Badge>(
      find.descendant(
        of: find.byTooltip('Inbox'),
        matching: find.byType(Badge),
      ),
    );

    testWidgets('the inbox button shows the unread count', (tester) async {
      final api = _api();
      final aid = api.seedArtifact('Runs', _page);
      api.pushInbox('reply', artifactId: aid, body: 'one');
      api.pushInbox('reply', artifactId: aid, body: 'two');
      await pumpWithInbox(tester, api);
      expect(badge(tester).isLabelVisible, isTrue);
      expect(
        find.descendant(of: find.byType(Badge), matching: find.text('2')),
        findsOneWidget,
      );
      final names = api.callNames;
      expect(
        names.indexOf('streamSubscribe'),
        lessThan(names.indexOf('inboxSummary')),
        reason: 'subscribed before the count is fetched',
      );
    });

    testWidgets('no unread items: no count on the button', (tester) async {
      await pumpWithInbox(tester, _api());
      expect(badge(tester).isLabelVisible, isFalse);
    });

    testWidgets('the count follows inbox events', (tester) async {
      final api = _api();
      final aid = api.seedArtifact('Runs', _page);
      await pumpWithInbox(tester, api);
      final item = api.pushInbox('reply', artifactId: aid, body: 'one');
      await tester.pumpAndSettle();
      expect(
        find.descendant(of: find.byType(Badge), matching: find.text('1')),
        findsOneWidget,
      );
      await api.markInboxRead(item.id);
      await tester.pumpAndSettle();
      expect(badge(tester).isLabelVisible, isFalse);
      api.emitStream(
        const StreamEvent(
          topic: 'inbox',
          type: 'inbox_read',
          data: {'topic': 'inbox', 'ids': null, 'read': true, 'unread': 7},
        ),
      );
      await tester.pumpAndSettle();
      expect(
        find.descendant(of: find.byType(Badge), matching: find.text('7')),
        findsOneWidget,
      );
    });

    testWidgets('a resync refetches the count', (tester) async {
      final api = _api();
      await pumpWithInbox(tester, api);
      final before = api.callNames.where((n) => n == 'inboxSummary').length;
      api.emitStream(
        const StreamEvent(
          topic: 'inbox',
          type: 'resync',
          data: {'topic': 'inbox', 'reason': 'reconnect'},
        ),
      );
      await tester.pumpAndSettle();
      expect(
        api.callNames.where((n) => n == 'inboxSummary').length,
        before + 1,
      );
    });

    testWidgets('the button opens the inbox', (tester) async {
      await pumpWithInbox(tester, _api());
      await tester.tap(find.byTooltip('Inbox'));
      await tester.pumpAndSettle();
      expect(find.byType(InboxScreen), findsOneWidget);
    });

    testWidgets(
      "a reply opens its app at the thread, a version at its number",
      (tester) async {
        final api = _api();
        final aid = api.seedArtifact('Runs', _page);
        api.pushInbox('reply', artifactId: aid, threadId: 't9', body: 'Fixed');
        api.pushInbox('version', artifactId: aid, versionN: 4, note: 'Totals');
        api.pushInbox('published', artifactId: aid);
        await pumpWithInbox(tester, api);
        await tester.tap(find.byTooltip('Inbox'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Fixed'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('v4: Totals'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('New app'));
        await tester.pumpAndSettle();
        expect(targets, [(aid, 't9', null), (aid, null, 4), (aid, null, null)]);
      },
    );

    testWidgets('without a target hook an item opens its app', (tester) async {
      final api = _api();
      final aid = api.seedArtifact('Runs', _page);
      api.pushInbox('reply', artifactId: aid, threadId: 't9', body: 'Fixed');
      await pumpWithInbox(tester, api, targetHook: false);
      await tester.tap(find.byTooltip('Inbox'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Fixed'));
      await tester.pumpAndSettle();
      expect(opened, [aid]);
    });

    testWidgets('a question goes to the question hook when there is one', (
      tester,
    ) async {
      final api = _api();
      final aid = api.seedArtifact('Runs', _page);
      final s = await api.registerAppSession(installId: 'i', pid: 1);
      final asked = await api.askQuestion(s.id, [
        {'question': 'Which unit?', 'header': 'Units'},
      ], artifactId: aid);
      await pumpWithInbox(tester, api, questionHook: true);
      await tester.tap(find.byTooltip('Inbox'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Units'));
      await tester.pumpAndSettle();
      expect(questionsOpened, [asked.question.id]);
      expect(targets, isEmpty);
    });

    testWidgets('a question without the hook opens its app', (tester) async {
      final api = _api();
      final aid = api.seedArtifact('Runs', _page);
      final s = await api.registerAppSession(installId: 'i', pid: 1);
      await api.askQuestion(s.id, [
        {'question': 'Which unit?', 'header': 'Units'},
      ], artifactId: aid);
      await pumpWithInbox(tester, api);
      await tester.tap(find.byTooltip('Inbox'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Units'));
      await tester.pumpAndSettle();
      expect(targets, [(aid, null, null)]);
    });

    testWidgets('an unsubscribed inbox is subscribed again with backoff', (
      tester,
    ) async {
      final api = _api();
      await pumpWithInbox(tester, api);
      final summaries = api.callNames.where((n) => n == 'inboxSummary').length;
      api.failNext(
        'streamSubscribe',
        const ClaxApiError(status: 503, code: 'unavailable', message: 'x'),
      );
      api.emitStream(
        const StreamEvent(
          topic: 'inbox',
          type: 'unsubscribed',
          data: {'topic': 'inbox', 'code': 'unavailable', 'message': 'x'},
        ),
      );
      await tester.pumpAndSettle();
      expect(api.callNames.where((n) => n == 'streamSubscribe').length, 3);
      expect(delays, [const Duration(seconds: 1)]);
      expect(
        api.callNames.where((n) => n == 'inboxSummary').length,
        summaries + 1,
      );
    });
  });
}

/// Timings that keep nothing.
class _NoTimings implements Timings {
  @override
  void noSuchMethod(Invocation i) {}
}
