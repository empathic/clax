import 'dart:async';
import 'dart:convert';

import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/platform/app_notice.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/queue/client.dart' hide Intent;
import 'package:clax_app/ui/app_cover.dart';
import 'package:clax_app/ui/app_icon.dart';
import 'package:clax_app/ui/app_menu.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/app_surface.dart';
import 'package:clax_app/ui/build_plan.dart';
import 'package:clax_app/ui/comment_bar.dart';
import 'package:clax_app/ui/bundle_sheets.dart';
import 'package:clax_app/ui/change_sheet.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/mac/mac_toolbar.dart';
import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:clax_app/ui/pill_strip.dart';
import 'package:clax_app/ui/shortcuts.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:clax_app/ui/working_chip.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:clax_app/ui/threads_panel.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_app_notice.dart';
import '../support/fake_queue.dart';
import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

/// A clock that stands still unless a test moves it: every gesture is
/// within its time window, and a delay ends only when [lapse] ends it.
class _Clock implements Clock {
  DateTime t = DateTime.utc(2026, 10, 12);
  final _delays = <Completer<void>>[];
  @override
  DateTime now() => t;
  @override
  Future<void> delay(Duration d) =>
      (_delays..add(Completer<void>())).last.future;

  /// Ends every delay asked for so far.
  void lapse() {
    for (final c in List.of(_delays)) {
      c.complete();
    }
    _delays.clear();
  }
}

/// A clock whose time moves only by [advance], which ends each delay that
/// falls due on the way.
class _StepClock implements Clock {
  DateTime t = DateTime.utc(2026, 10, 12);
  final _delays = <(DateTime, Completer<void>)>[];
  @override
  DateTime now() => t;
  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _delays.add((t.add(d), c));
    return c.future;
  }

  void advance(Duration d) {
    t = t.add(d);
    for (final (at, c) in List.of(_delays)) {
      if (!at.isAfter(t)) {
        c.complete();
        _delays.remove((at, c));
      }
    }
  }
}

/// Timings that log what the screen asked of them: the send→first-step
/// timing in [stepLog], the others in [log].
class _Timings implements Timings {
  final log = <String>[];
  final extras = <Map<String, Object?>>[];
  final stepLog = <String>[];

  @override
  void start(String name) =>
      (name == sendFirstStep ? stepLog : log).add('start $name');

  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) {
    if (name == sendFirstStep) {
      stepLog.add('stop $name ${extra['artifact']}');
      return null;
    }
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

/// A 1 x 1 PNG.
final _png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
);
final _base = Uri.parse('http://127.0.0.1:7999');

class _Rig {
  _Rig(this.api, this.aid, this.surface, this.voice, this.timings);

  final FakeClaxApi api;
  final String aid;
  final FakeAppSurface surface;
  final FakeVoice voice;
  final _Timings timings;
  final worker = StreamController<WorkerEvent>.broadcast(sync: true);

  /// The app's Comment and Threads actions (the notice's, or the menu's).
  final actions = StreamController<AppAction>.broadcast(sync: true);

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

/// A build that runs, as the worker reports it.
class _Builds implements BuildControl {
  final _c = StreamController<BuildState>.broadcast(sync: true);
  final states = <String, BuildState>{};

  void emit(BuildState s) {
    states[s.appId] = s;
    _c.add(s);
  }

  @override
  Stream<BuildState> get builds => _c.stream;
  @override
  BuildState? buildOf(String appId) => states[appId];
  @override
  Future<void> stopBuild(String appId) async {}
  @override
  Future<void> resumeBuild(String appId) async {}
}

/// The rig [pumpScreen] made last.
late _Rig _current;

/// The Clax pill saying [state] (anything, when null).
Finder _pill([ClaxPillState? state]) => find.byWidgetPredicate(
  (w) => w is ClaxPill && (state == null ? w.state != null : w.state == state),
);

final _working = _pill(ClaxPillState.working);

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
    CommentsCommand? comments,
    FakeAppNotice? notice,
    AppAction? initialAction,
    ValueChanged<String>? onRallied,
    ValueChanged<bool>? onDictateChanged,
    ValueChanged<double>? onCommentTabMoved,
    Future<bool> Function(String aid, String tid)? onRetry,
    void Function(String aid)? onStop,
    QueueClient? queue,
    Future<bool> Function(String aid, String tid)? onCancelThread,
    bool Function(String tid)? isStopped,
    String? initialThread,
    String? initialComment,
    Clock? clock,
    Future<(FakeClaxApi, String)> Function()? seed,
    bool pushed = false,
    bool developer = false,
    Future<void> Function(BuildContext, BundleFiles)? share,
    VoidCallback? leave,
    ValueChanged<double>? onThreadsWidth,
    BuildLink? build,
    VoidCallback? onInspect,
  }) async {
    phone(tester, size: size);
    final (api, aid) = await (seed ?? _seeded)();
    final rig = _Rig(
      api,
      aid,
      FakeAppSurface(),
      voice ?? FakeVoice(script: const ['make it', 'make it blue'], hold: true),
      _Timings(),
    );
    _current = rig;
    final screen = AppScreen(
      aid: aid,
      base: _base,
      api: api,
      surface: rig.surface,
      voice: rig.voice,
      settings: settings ?? AppSettings(),
      timings: rig.timings,
      worker: rig.worker.stream,
      clock: clock ?? _Clock(),
      comments: comments,
      notice: notice == null ? null : AppNoticeSlot(notice),
      actions: rig.actions.stream,
      initialAction: initialAction,
      onRallied: onRallied,
      onDictateChanged: onDictateChanged,
      onCommentTabMoved: onCommentTabMoved,
      onRetry: onRetry,
      onStop: onStop,
      queue: queue,
      onCancelThread: onCancelThread,
      isStopped: isStopped,
      initialThread: initialThread,
      initialComment: initialComment,
      share: share,
      leave: leave,
      onThreadsWidth: onThreadsWidth,
      build: build,
      onInspect: onInspect,
    );
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
        builder: (_, child) => DeveloperScope(
          notifier: ValueNotifier(developer),
          child: child!,
        ),
        // [pushed]: over a launcher, as the app opens it.
        home: pushed ? const Scaffold(body: Text('Launcher')) : screen,
      ),
    );
    if (pushed) {
      unawaited(
        tester
            .state<NavigatorState>(find.byType(Navigator))
            .push(MaterialPageRoute<void>(builder: (_) => screen)),
      );
      // The cover's spinner never settles: past the route's transition.
      await tester.pump();
      await tester.pump(const Duration(seconds: 1));
    }
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

  /// Opens the threads panel as the notice's Threads action does.
  Future<void> openPanel(WidgetTester tester) async {
    await tester.pump();
    _current.actions.add(AppAction.threads);
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

  testWidgets('the Threads action opens the panel and back closes it', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await tester.pump();
    await openPanel(tester);
    expect(find.byKey(const ValueKey('app-panel')), findsOneWidget);
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

  testWidgets(
    'on the Mac at expanded widths the panel is a 320 pane beside the page, '
    'which its edge resizes within 280 to 420 and keeps',
    (tester) async {
      final kept = <double>[];
      final rig = await pumpScreen(
        tester,
        size: const Size(1280, 800),
        onThreadsWidth: kept.add,
      );
      rig.surface.push(rig.ready());
      await openPanel(tester);
      final pane = find.byKey(const ValueKey('app-panel'));
      final page = find.byKey(const ValueKey('fake-surface'));
      expect(tester.getRect(pane).width, 320);
      expect(tester.getRect(pane).right, 1280);
      expect(tester.getRect(page).right, closeTo(tester.getRect(pane).left, 0.01));
      final edge = find.byKey(const ValueKey('threads-pane-edge'));
      await tester.drag(edge, const Offset(-200, 0));
      await tester.pumpAndSettle();
      expect(tester.getRect(pane).width, 420);
      expect(tester.getRect(page).right, closeTo(tester.getRect(pane).left, 0.01));
      await tester.drag(edge, const Offset(300, 0));
      await tester.pumpAndSettle();
      expect(tester.getRect(pane).width, 280);
      expect(kept, [420, 280]);
      // Closed before the test's platform goes, so the window's Escape is
      // given back.
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(pane, findsNothing);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets(
    'on the Mac the pane opens at the width the settings keep',
    (tester) async {
      final rig = await pumpScreen(
        tester,
        size: const Size(1280, 800),
        settings: AppSettings(threadsWidth: 400),
      );
      rig.surface.push(rig.ready());
      await openPanel(tester);
      expect(
        tester.getRect(find.byKey(const ValueKey('app-panel'))).width,
        400,
      );
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets(
    'on the Mac at 1200 the composer is a 360 popover under the pick, over '
    'the page; Return sends and Shift-Return starts a new line',
    (tester) async {
      final rig = await pumpScreen(
        tester,
        size: const Size(1200, 800),
        settings: AppSettings(dictateComments: false),
      );
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.push(
        const StateMsg(
          commenting: false,
          draft: Draft(
            label: 'h1',
            text: '',
            clip: DraftClip.pending,
            at: Rect.fromLTWH(100, 200, 300, 40),
          ),
          busy: false,
        ),
      );
      await tester.pumpAndSettle();
      final page = tester.getRect(find.byKey(const ValueKey('fake-surface')));
      final pop = find.byKey(const ValueKey('composer-popover'));
      expect(pop, findsOneWidget);
      expect(find.byKey(const ValueKey('app-panel')), findsNothing);
      expect(find.byKey(const ValueKey('composer-sheet')), findsNothing);
      final box = tester.getRect(pop);
      expect(box.width, 360);
      // Under the pick, centred on it.
      expect(box.top, page.top + 240 + 8);
      expect(box.center.dx, closeTo(page.left + 250, 0.01));
      // The page keeps its size.
      expect(page.width, 1200);

      final field = find.byKey(const ValueKey('composer-field'));
      await tester.tap(field);
      await tester.enterText(field, 'make it');
      await tester.pump();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shift);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shift);
      await tester.pump();
      final text = tester
          .widget<EditableText>(
            find.descendant(of: field, matching: find.byType(EditableText)),
          )
          .controller;
      expect(text.text, 'make it\n');
      expect(
        rig.surface.js.where((j) => j.contains('submitDraft')),
        isEmpty,
      );
      text.text = 'make it blue';
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(
        rig.surface.js.last,
        'window.claxHost?.submitDraft({text:"make it blue"})',
      );
      // Closed before the test's platform goes, so the window's Escape is
      // given back.
      rig.surface.push(_Rig.noDraft);
      await tester.pumpAndSettle();
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets(
    'the popover goes above a pick near the page\'s bottom, stays inside the '
    'page, and sits at the top without a pick',
    (tester) async {
      final rig = await pumpScreen(
        tester,
        size: const Size(1200, 800),
        settings: AppSettings(dictateComments: false),
      );
      rig.surface.push(rig.ready());
      await tester.pump();
      StateMsg at(Rect? r) => StateMsg(
        commenting: false,
        draft: Draft(label: 'h1', text: '', clip: DraftClip.pending, at: r),
        busy: false,
      );
      rig.surface.push(at(const Rect.fromLTWH(1100, 740, 80, 30)));
      await tester.pumpAndSettle();
      final page = tester.getRect(find.byKey(const ValueKey('fake-surface')));
      var box = tester.getRect(find.byKey(const ValueKey('composer-popover')));
      expect(box.bottom, page.top + 740 - 8);
      expect(box.right, page.right - 8);
      rig.surface.push(_Rig.noDraft);
      await tester.pumpAndSettle();
      rig.surface.push(at(null));
      await tester.pumpAndSettle();
      box = tester.getRect(find.byKey(const ValueKey('composer-popover')));
      expect(box.top, page.top + 8);
      expect(box.center.dx, closeTo(page.center.dx, 0.01));
      rig.surface.push(_Rig.noDraft);
      await tester.pumpAndSettle();
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets(
    'a pick scrolled above the page keeps the popover at the page\'s top, '
    'and one scrolled below it keeps the popover at its bottom',
    (tester) async {
      final rig = await pumpScreen(
        tester,
        size: const Size(1200, 800),
        settings: AppSettings(dictateComments: false),
      );
      rig.surface.push(rig.ready());
      await tester.pump();
      StateMsg at(Rect r) => StateMsg(
        commenting: false,
        draft: Draft(label: 'h1', text: '', clip: DraftClip.pending, at: r),
        busy: false,
      );
      final pop = find.byKey(const ValueKey('composer-popover'));
      rig.surface.push(at(const Rect.fromLTWH(100, -500, 300, 40)));
      await tester.pumpAndSettle();
      final page = tester.getRect(find.byKey(const ValueKey('fake-surface')));
      var box = tester.getRect(pop);
      expect(box.top, page.top + 8);
      expect(box.center.dx, closeTo(page.left + 250, 0.01));
      rig.surface.push(at(const Rect.fromLTWH(100, 1500, 300, 40)));
      await tester.pumpAndSettle();
      box = tester.getRect(pop);
      expect(box.bottom, page.bottom - 8);
      expect(box.top, greaterThanOrEqualTo(page.top + 8));
      rig.surface.push(_Rig.noDraft);
      await tester.pumpAndSettle();
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets(
    "a pick docks the app's composer and dictates into its field; its Send sends the text; the timing waits for Send",
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      final page = find.byKey(const ValueKey('fake-surface'));
      rig.surface.push(_Rig.draft);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('composer-sheet')), findsOneWidget);
      expect(find.byKey(const ValueKey('app-panel')), findsNothing);
      expect(find.text('Versions'), findsNothing);
      // The composer sits at the bottom, over the page's lower edge.
      final sheet = tester.getRect(find.byKey(const ValueKey('composer')));
      expect(sheet.bottom, 780);
      expect(sheet.top, greaterThan(tester.getRect(page).top + 300));
      expect(rig.timings.log, isEmpty, reason: 'it starts at Send (spec 6.2)');
      expect(rig.voice.listens, 1);
      expect(
        rig.surface.js,
        containsAllInOrder([
          'window.claxHost?.setDraftText("make it",{composer:true})',
          'window.claxHost?.setDraftText("make it blue",{composer:true})',
        ]),
      );
      expect(find.text('make it blue'), findsOneWidget);

      // The mic button stops and starts listening.
      await tester.tap(find.byKey(const ValueKey('dictate')));
      await tester.pumpAndSettle();
      expect(rig.voice.listening, isFalse);
      expect(rig.voice.stops, 1);
      await tester.tap(find.byKey(const ValueKey('dictate')));
      await tester.pumpAndSettle();
      expect(rig.voice.listens, 2);

      // One Send, the composer's: it sends the field's text.
      expect(find.text('Send'), findsOneWidget);
      await tester.enterText(
        find.byKey(const ValueKey('composer-field')),
        'make it blue please',
      );
      await tester.pump();
      expect(
        rig.surface.js.last,
        'window.claxHost?.setDraftText("make it blue please",{composer:true})',
      );
      await tester.tap(find.byKey(const ValueKey('send')));
      await tester.pump();
      expect(
        rig.surface.js.last,
        'window.claxHost?.submitDraft({text:"make it blue please"})',
      );
      expect(find.text('Sending…'), findsOneWidget);
      expect(
        tester
            .widget<ButtonStyleButton>(find.byKey(const ValueKey('send')))
            .onPressed,
        isNull,
        reason: 'one Send per comment',
      );

      // The composer closing ends the composer and the listen.
      rig.surface.push(_Rig.noDraft);
      await tester.pumpAndSettle();
      expect(rig.voice.listening, isFalse);
      expect(find.byKey(const ValueKey('composer')), findsNothing);
      expect(find.text('Versions'), findsOneWidget);
    },
  );

  testWidgets(
    'Cancel and back close the composer; a failed post may send again',
    (tester) async {
      final rig = await pumpScreen(
        tester,
        settings: AppSettings(dictateComments: false),
      );
      rig.surface.push(rig.ready());
      await openPanel(tester);
      rig.surface.push(_Rig.draft);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('composer-cancel')));
      expect(rig.surface.js.last, 'window.claxHost?.cancelDraft()');
      await tester.binding.handlePopRoute();
      expect(rig.surface.js.last, 'window.claxHost?.cancelDraft()');
      expect(find.byKey(const ValueKey('composer')), findsOneWidget);

      await tester.enterText(
        find.byKey(const ValueKey('composer-field')),
        'bigger',
      );
      await tester.tap(find.byKey(const ValueKey('send')));
      await tester.pump();
      rig.surface.push(
        const StateMsg(
          commenting: false,
          draft: Draft(label: 'h1', text: 'bigger', clip: DraftClip.pending),
          busy: true,
        ),
      );
      rig.surface.push(const Notice('Could not post the comment'));
      rig.surface.push(
        const StateMsg(
          commenting: false,
          draft: Draft(label: 'h1', text: 'bigger', clip: DraftClip.pending),
          busy: false,
        ),
      );
      await tester.pump();
      expect(find.text('bigger'), findsOneWidget, reason: 'the text is kept');
      expect(
        tester
            .widget<ButtonStyleButton>(find.byKey(const ValueKey('send')))
            .onPressed,
        isNotNull,
      );
    },
  );

  group('the keyboard comes up over the page, and the composer rides above it', () {
    const range = StateMsg(
      commenting: false,
      draft: Draft(
        key: 'r1',
        kind: 'range',
        label: '“Grow revenue in every region”',
        quote:
            'Grow revenue in every region this year, and hire two more '
            'people for the support team',
        text: '',
        clip: DraftClip.ready,
      ),
      busy: false,
    );
    for (final (size, scale) in [
      (const Size(320, 568), 1.0),
      (const Size(320, 568), 1.3),
      (const Size(390, 844), 1.0),
      (const Size(390, 844), 1.3),
      (const Size(1100, 720), 1.0),
      (const Size(1100, 720), 1.3),
    ]) {
      testWidgets(
        '${size.width.toInt()} x ${size.height.toInt()} dp at text scale $scale',
        (tester) async {
          final rig = await pumpScreen(
            tester,
            size: size,
            settings: AppSettings(dictateComments: false),
          );
          tester.platformDispatcher.textScaleFactorTestValue = scale;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
          rig.surface.push(rig.ready());
          await openPanel(tester);
          rig.surface.push(range);
          rig.surface.push(DraftClipMsg('r1', _png));
          await tester.pumpAndSettle();
          final page = find.byKey(const ValueKey('fake-surface'));
          final before = tester.getRect(page);
          // The field has focus as the composer opens (no dictation).
          expect(
            tester
                .widget<TextField>(
                  find.descendant(
                    of: find.byKey(const ValueKey('composer-field')),
                    matching: find.byType(TextField),
                  ),
                )
                .focusNode!
                .hasFocus,
            isTrue,
          );
          // A 260 dp keyboard.
          tester.view.viewInsets = const FakeViewPadding(bottom: 260 * 3);
          addTearDown(tester.view.resetViewInsets);
          await tester.pumpAndSettle();
          expect(tester.takeException(), isNull);
          expect(
            tester.getRect(page),
            before,
            reason: 'the page keeps its size',
          );
          final kb = size.height - 260;
          for (final k in ['send', 'composer-cancel', 'composer-field']) {
            final r = tester.getRect(find.byKey(ValueKey(k)));
            expect(
              r.bottom,
              lessThanOrEqualTo(kb),
              reason: '$k above the keyboard',
            );
            expect(
              r.top,
              greaterThanOrEqualTo(24),
              reason: '$k under the status bar',
            );
          }
          expect(find.byKey(const ValueKey('snippet-clip')), findsOneWidget);
          if (size.width < 840) {
            expect(
              tester.getRect(find.byKey(const ValueKey('composer'))).bottom,
              kb,
              reason: 'it rides on the keyboard',
            );
          }
        },
      );
    }
  });

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
      'window.claxHost?.setDraftText("make it blue",{composer:true})',
    );
    expect(find.text('make it blue'), findsOneWidget);
  });

  testWidgets('a dictation result after Send leaves the sent text alone', (
    tester,
  ) async {
    // The recognizer's final words land after Send stops it: written into
    // the field, they would reach the shell as an edit, which drops a post
    // queued behind the screenshot without a word, and Sending… would stay.
    final rig = await pumpScreen(
      tester,
      voice: FakeVoice(
        script: const ['make it'],
        hold: true,
        atStop: 'make it blue',
      ),
    );
    rig.surface.push(rig.ready());
    await openPanel(tester);
    rig.surface.push(_Rig.draft);
    await tester.pumpAndSettle();
    expect(find.text('make it'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('send')));
    await tester.pumpAndSettle();
    expect(rig.voice.stops, 1);
    expect(
      rig.surface.js.where((j) => j.contains('claxHost?.setDraftText')).last,
      'window.claxHost?.setDraftText("make it",{composer:true})',
    );
    expect(
      rig.surface.js.last,
      'window.claxHost?.submitDraft({text:"make it"})',
    );
    expect(find.text('make it'), findsOneWidget);
    expect(find.text('Sending…'), findsOneWidget);
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
      expect(_working, findsOneWidget);
      expect(rig.voice.listening, isFalse);

      rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
      rig.worker.add(WorkerLanded(rig.aid, 4));
      await tester.pump();
      // The New version toast takes the pill's place until it goes.
      expect(find.byKey(const ValueKey('newer-toast')), findsOneWidget);
      expect(_pill(), findsNothing);

      rig.surface.push(const Landed(4));
      await tester.pump();
      expect(_working, findsNothing);
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
    rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
    rig.worker.add(WorkerReplied(rig.aid));
    await tester.pump();
    expect(_working, findsNothing);
    await tester.pump(const Duration(milliseconds: 500));
    expect(find.text('Clax replied'), findsOneWidget);
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
      expect(_working, findsNothing);
      expect(find.byKey(const ValueKey('chip-failed')), findsOneWidget);
      expect(find.textContaining('The API key was refused'), findsOneWidget);

      rig.surface.push(const Sent('thread-2'));
      await tester.pump();
      expect(_working, findsOneWidget);
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
    expect(_working, findsOneWidget);
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
      expect(_working, findsNothing);
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
    expect(find.textContaining('Tap anything'), findsOneWidget);
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
    expect(find.textContaining('On the page title · Version'), findsOneWidget);
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
    expect(find.byKey(const ValueKey('failure-reply')), findsOneWidget);
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
    'a reloading page is covered again; one that gave up offers Try again',
    (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.fault(const SurfaceFault('gone', reloading: true));
      await tester.pump();
      expect(find.byKey(const ValueKey('app-cover')), findsOneWidget);
      expect(find.text('Try again'), findsNothing);
      rig.surface.fault(const SurfaceFault('gone again', reloading: false));
      await tester.pump();
      expect(find.text('Try again'), findsOneWidget);
      await tester.tap(find.text('Try again'));
      await tester.pump();
      expect(rig.surface.reloads, 1);
      expect(find.text('Try again'), findsNothing);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(find.byKey(const ValueKey('app-cover')), findsNothing);
    },
  );

  group('the loading cover', () {
    final cover = find.byKey(const ValueKey('app-cover'));
    final mark = find.byKey(const ValueKey('app-cover-mark'));

    /// Moves [clock] on by [ms] and lets the screen catch up.
    Future<void> after(WidgetTester tester, _StepClock clock, int ms) async {
      clock.advance(Duration(milliseconds: ms));
      await tester.pump();
      await tester.pump();
    }

    testWidgets('is the plain surface for 100 ms, then the working mark at '
        'its centre', (tester) async {
      final clock = _StepClock();
      await pumpScreen(tester, clock: clock);
      expect(cover, findsOneWidget);
      expect(mark, findsNothing);
      expect(find.descendant(of: cover, matching: find.byType(AppBadge)),
          findsNothing);
      expect(find.descendant(of: cover, matching: find.text('Runs')),
          findsNothing);
      await after(tester, clock, 99);
      expect(mark, findsNothing);
      await after(tester, clock, 1);
      expect(mark, findsOneWidget);
      expect(tester.widget<ClaxMark>(mark).state, ClaxMarkState.working);
      expect(tester.getCenter(mark), tester.getCenter(cover));
      expect(AppCover.markAfter, const Duration(milliseconds: 100));
      expect(AppCover.markHold, ClaxMotion.markBreath);
    });

    testWidgets('a page up at 50 ms never shows the mark', (tester) async {
      final clock = _StepClock();
      final rig = await pumpScreen(tester, clock: clock);
      await after(tester, clock, 50);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(cover, findsNothing);
      await after(tester, clock, 2000);
      expect(cover, findsNothing);
      expect(mark, findsNothing);
    });

    testWidgets('a page up at 150 ms keeps the cover until the mark’s first '
        'breath has ended', (tester) async {
      final clock = _StepClock();
      final rig = await pumpScreen(tester, clock: clock);
      await after(tester, clock, 100);
      await after(tester, clock, 50);
      expect(mark, findsOneWidget);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(cover, findsOneWidget);
      expect(mark, findsOneWidget);
      // The mark showed at 100 ms: its breath ends at 1700 ms.
      await after(tester, clock, 1549);
      expect(cover, findsOneWidget);
      await after(tester, clock, 1);
      expect(cover, findsNothing);
      expect(find.byKey(const ValueKey('fake-surface')), findsOneWidget);
    });

    testWidgets('a page up after a whole breath lifts it at once',
        (tester) async {
      final clock = _StepClock();
      final rig = await pumpScreen(tester, clock: clock);
      await after(tester, clock, 100);
      await after(tester, clock, 1700);
      expect(mark, findsOneWidget);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(cover, findsNothing);
    });

    testWidgets('under reduced motion the still mark is not held',
        (tester) async {
      tester.platformDispatcher.accessibilityFeaturesTestValue =
          const FakeAccessibilityFeatures(disableAnimations: true);
      addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
      final clock = _StepClock();
      final rig = await pumpScreen(tester, clock: clock);
      await after(tester, clock, 150);
      expect(mark, findsOneWidget);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(cover, findsNothing);
    });

    testWidgets('a reload while the cover waits out the breath keeps it and '
        'its mark', (tester) async {
      final clock = _StepClock();
      final rig = await pumpScreen(tester, clock: clock);
      await after(tester, clock, 150);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.fault(const SurfaceFault('gone', reloading: true));
      await tester.pump();
      await after(tester, clock, 2000);
      expect(cover, findsOneWidget);
      expect(mark, findsOneWidget);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(cover, findsNothing);
    });
  });

  testWidgets('a page that gave up says so plainly with the app’s icon, Try '
      'again and Back to Home; its raw text is under Details only with '
      'Developer tools on', (tester) async {
    for (final developer in [false, true]) {
      final rig = await pumpScreen(tester, developer: developer, pushed: true);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.fault(
        const SurfaceFault('net::ERR_CONNECTION_REFUSED', reloading: false),
      );
      await tester.pump();
      final cover = find.byKey(const ValueKey('app-cover'));
      expect(find.descendant(of: cover, matching: find.byType(ClaxEmpty)),
          findsOneWidget);
      expect(find.descendant(of: cover, matching: find.byType(AppBadge)),
          findsOneWidget);
      for (final t in [
        'This app didn’t load',
        'Clax’s service didn’t answer.',
        'Try again',
        'Back to Home',
      ]) {
        expect(find.text(t), findsOneWidget, reason: t);
      }
      for (final t in ['This app didn’t load', 'Clax’s service didn’t answer.']) {
        expect(tester.widget<Text>(find.text(t)).textAlign, TextAlign.center);
      }
      expect(find.textContaining('net::ERR'), findsNothing);
      expect(find.text('Details'), developer ? findsOneWidget : findsNothing);
      if (developer) {
        await tester.tap(find.text('Details'));
        await tester.pump();
        expect(find.text('net::ERR_CONNECTION_REFUSED'), findsOneWidget);
      }
      await tester.tap(find.text('Back to Home'));
      await tester.pumpAndSettle();
      expect(find.text('Launcher'), findsOneWidget);
    }
  });

  testWidgets('the cover offers no Back to Home with nothing to go back to', (
    tester,
  ) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    await tester.pump();
    rig.surface.fault(const SurfaceFault('gone', reloading: false));
    await tester.pump();
    expect(find.text('Try again'), findsOneWidget);
    expect(find.text('Back to Home'), findsNothing);
  });

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

  group('narration and Retry', () {
    testWidgets(
      "the chip shows the running change's latest step and elapsed time",
      (tester) async {
        final clock = _Clock();
        final rig = await pumpScreen(tester, clock: clock);
        rig.surface.push(rig.ready());
        rig.surface.push(const Sent('thread-1'));
        rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
        await tester.pump();
        expect(_working, findsOneWidget);
        expect(rig.timings.stepLog, ['start $sendFirstStep']);

        rig.worker.add(
          WorkerProgress(rig.aid, const [
            'thread-1',
          ], const ToolStarted('str_replace', 'Changing index.html')),
        );
        rig.worker.add(
          WorkerProgress(rig.aid, const [
            'thread-1',
          ], const Narration('Adding the weekly total')),
        );
        clock.t = clock.t.add(const Duration(seconds: 9));
        await tester.pump(const Duration(seconds: 1));
        // The pill says Working; the steps are the change sheet's.
        expect(_working, findsOneWidget);
        expect(find.text('Adding the weekly total'), findsNothing);
        expect(rig.timings.stepLog, [
          'start $sendFirstStep',
          'stop $sendFirstStep ${rig.aid}',
        ]);

        await tester.tap(_working);
        await frames(tester);
        expect(find.text('Changing index.html'), findsOneWidget);

        rig.worker.add(WorkerLanded(rig.aid, 4));
        await tester.pump();
        expect(find.byKey(const ValueKey('chip-expanded')), findsNothing);
        rig.surface.push(const Landed(4));
        await tester.pump();
        expect(_working, findsNothing);
      },
    );

    testWidgets("another thread's or app's steps are ignored", (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
      rig.worker.add(
        WorkerProgress(rig.aid, const ['thread-2'], const Narration('Other')),
      );
      rig.worker.add(
        WorkerProgress('other', const ['thread-1'], const Narration('Else')),
      );
      await tester.pump();
      expect(_working, findsOneWidget);
      expect(find.text('Other'), findsNothing);
      expect(find.text('Else'), findsNothing);
    });

    testWidgets(
      'a failed change offers Retry, which asks the worker and runs again',
      (tester) async {
        final asked = <String>[];
        final answer = Completer<bool>();
        final rig = await pumpScreen(
          tester,
          onRetry: (aid, tid) {
            asked.add('$aid/$tid');
            return answer.future;
          },
        );
        rig.surface.push(rig.ready());
        rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
        rig.worker.add(WorkerFailed(rig.aid, 'the connection dropped'));
        await tester.pump();
        expect(find.textContaining('the connection dropped'), findsOneWidget);

        await tester.tap(find.byKey(const ValueKey('retry-change')));
        await tester.pump();
        expect(asked, ['${rig.aid}/thread-1']);
        expect(find.text('Retrying…'), findsOneWidget);
        await tester.tap(find.byKey(const ValueKey('retry-change')));
        expect(asked, hasLength(1), reason: 'one Retry at a time');

        answer.complete(true);
        await tester.pump();
        expect(find.byKey(const ValueKey('chip-failed')), findsNothing);
        expect(_working, findsOneWidget);

        rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
        rig.worker.add(
          WorkerProgress(rig.aid, const [
            'thread-1',
          ], const Narration('Changing the colour')),
        );
        await tester.pump();
        expect(_working, findsOneWidget);
        expect(rig.timings.stepLog, isEmpty, reason: 'a Retry is not a Send');
      },
    );

    testWidgets('a Retry the worker refuses says so', (tester) async {
      final rig = await pumpScreen(tester, onRetry: (_, _) async => false);
      rig.surface.push(rig.ready());
      rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
      rig.worker.add(WorkerFailed(rig.aid, 'x'));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('retry-change')));
      await tester.pump();
      expect(find.textContaining('Nothing to run again'), findsOneWidget);
      expect(find.byKey(const ValueKey('retry-change')), findsNothing);
    });

    testWidgets('with the queue, a failed change\'s Retry is a Retry of its '
        'job, which shows running again once the queue has it', (tester) async {
      final q = FakeQueueClient();
      final rig = await pumpScreen(tester, queue: q);
      rig.surface.push(rig.ready());
      rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
      q.push([changeJob('c1', rig.aid)]);
      rig.worker.add(WorkerFailed(rig.aid, 'it broke'));
      q.push([changeJob('c1', rig.aid, state: JobState.failed)]);
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('retry-change')));
      await tester.pump();
      expect(q.sent, [const Retry('c1')]);
      expect(find.text('Retrying…'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('retry-change')));
      expect(q.sent, hasLength(1), reason: 'one Retry at a time');
      q.push([changeJob('c1', rig.aid, state: JobState.queued)]);
      await tester.pump();
      expect(find.byKey(const ValueKey('chip-failed')), findsNothing);
      expect(_working, findsOneWidget);
    });

    testWidgets('with the queue, a Retry its job no longer offers says so', (
      tester,
    ) async {
      final q = FakeQueueClient();
      final rig = await pumpScreen(tester, queue: q);
      rig.surface.push(rig.ready());
      rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
      rig.worker.add(WorkerFailed(rig.aid, 'x'));
      q.push([changeJob('c1', rig.aid, state: JobState.failed)]);
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('retry-change')));
      q.push([
        changeJob('c1', rig.aid, state: JobState.failed, offers: {Offer.discard}),
      ]);
      await tester.pump();
      expect(find.textContaining('Nothing to run again'), findsOneWidget);
      expect(find.byKey(const ValueKey('retry-change')), findsNothing);
    });

    testWidgets('without a retry hook a failed change offers no Retry', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
      rig.worker.add(WorkerFailed(rig.aid, 'x'));
      await tester.pump();
      expect(find.byKey(const ValueKey('chip-failed')), findsOneWidget);
      expect(find.byKey(const ValueKey('retry-change')), findsNothing);
    });
  });

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
    expect(_working, findsNothing);
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
    expect(find.byKey(const ValueKey('newer-toast')), findsOneWidget);
    rig.surface.fault(const SurfaceFault('gone', reloading: true));
    rig.surface.push(rig.ready(version: 4, latest: 4));
    await tester.pump();
    expect(_working, findsNothing);
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
    rig.actions.add(AppAction.threads);
    await frames(tester);
    rig.surface.push(const Sent('thread-1'));
    rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
    await tester.tap(find.text('Versions'));
    await frames(tester);
    await tester.tap(find.byKey(const ValueKey('undo')));
    await frames(tester);
    rig.surface.push(const Landed(4));
    await tester.pump();
    expect(_working, findsOneWidget);
    expect(rig.timings.log, ['start $longpressLanded']);
  });

  testWidgets('Working is a pill with no dismiss; how a change ended can be '
      'dismissed', (tester) async {
    final rig = await pumpScreen(tester);
    rig.surface.push(rig.ready());
    rig.surface.push(const Sent('thread-1'));
    await tester.pump();
    expect(_working, findsOneWidget);
    expect(find.byKey(const ValueKey('chip-dismiss')), findsNothing);
    rig.worker.add(WorkerWorking(rig.aid, const ['thread-1']));
    rig.worker.add(WorkerCancelled(rig.aid));
    await tester.pump();
    expect(_working, findsNothing);
    expect(find.byKey(const ValueKey('chip-stopped')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('chip-dismiss')));
    await tester.pump();
    expect(find.byKey(const ValueKey('chip-stopped')), findsNothing);
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
      rig.actions.add(AppAction.threads);
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
      // The panel goes; the composer stays until it is sent or cancelled.
      await twoFingerSwipe(tester, 120);
      expect(find.byKey(const ValueKey('composer')), findsOneWidget);
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

  group('comment mode and the notice', () {
    final bar = find.byKey(const ValueKey('comment-bar'));
    final panel = find.byKey(const ValueKey('app-panel'));
    final handle = find.byKey(const ValueKey('comment-tab'));
    List<String> modes(_Rig rig) =>
        rig.surface.js.where((j) => j.contains('CommentMode')).toList();
    const enter = 'window.claxHost?.enterCommentMode()';
    const exit = 'window.claxHost?.exitCommentMode()';

    Future<void> comment(WidgetTester tester, _Rig rig) async {
      rig.actions.add(AppAction.comment);
      await tester.pumpAndSettle();
    }

    testWidgets('no Clax control is drawn over the app while notices are '
        'allowed', (tester) async {
      final rig = await pumpScreen(tester, notice: FakeAppNotice());
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('comment-button')), findsNothing);
      expect(handle, findsNothing);
      expect(bar, findsNothing);
      expect(find.byType(Badge), findsNothing);
    });

    testWidgets('without a notice (macOS) there is no handle', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(handle, findsNothing);
    });

    testWidgets('with notifications on the Comment tab shows only with Show '
        'the Comment tab', (tester) async {
      var rig = await pumpScreen(tester, notice: FakeAppNotice());
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(handle, findsNothing);

      rig = await pumpScreen(
        tester,
        notice: FakeAppNotice(),
        settings: AppSettings(showCommentTab: true),
      );
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(handle, findsOneWidget);
    });

    testWidgets('the Comment tab sits at the saved place, outside the '
        'back-gesture zone, and a move is told', (tester) async {
      final moved = <double>[];
      final rig = await pumpScreen(
        tester,
        notice: FakeAppNotice(allow: false),
        settings: AppSettings(commentTabAt: 0.4),
        onCommentTabMoved: moved.add,
      );
      tester.view.systemGestureInsets = const FakeViewPadding(right: 24 * 3);
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      final box = tester.getRect(handle);
      expect(box.right, 360 - 28);
      // A fraction of the page's height, under the 24 dp status bar.
      expect(box.center.dy, closeTo(24 + 756 * 0.4, 1));
      final g = await tester.startGesture(box.center);
      await tester.pump(kLongPressTimeout + const Duration(milliseconds: 50));
      await g.moveBy(const Offset(0, 78));
      await tester.pump();
      await g.up();
      await tester.pumpAndSettle();
      expect(moved.single, closeTo((756 * 0.4 + 78) / 756, 0.001));
      expect(tester.getRect(handle).center.dy, closeTo(box.center.dy + 78, 1));
    });

    testWidgets("the Comment tab's Comment turns comment mode on with the "
        'bar, and the tab goes while it is on', (tester) async {
      final rig = await pumpScreen(tester, notice: FakeAppNotice(allow: false));
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      await tester.tap(handle);
      await tester.pumpAndSettle();
      expect(find.text('Threads (0)'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('comment')));
      await tester.pumpAndSettle();
      expect(bar, findsOneWidget);
      expect(modes(rig), [enter]);
      expect(handle, findsNothing);
    });

    testWidgets('with notifications off the Comment tab is the way in, until '
        'they are allowed', (tester) async {
      final notice = FakeAppNotice(allow: false);
      final rig = await pumpScreen(tester, notice: notice);
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(notice.calls, ['allowed'], reason: 'nothing is posted');
      await tester.tap(handle);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('threads')));
      await tester.pumpAndSettle();
      expect(panel, findsOneWidget);
      expect(modes(rig), [enter]);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(handle, findsOneWidget);

      // Allowed in system settings meanwhile: checked again on return.
      notice.allow = true;
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pumpAndSettle();
      expect(handle, findsNothing);
      expect(notice.shown, '${rig.aid} Runs');
    });

    testWidgets('a swiped-away notice leaves the Comment tab until the screen '
        'next comes to the front, which posts it again', (tester) async {
      final notice = FakeAppNotice();
      final rig = await pumpScreen(tester, notice: notice);
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(handle, findsNothing);
      notice.dismiss();
      await tester.pumpAndSettle();
      expect(handle, findsOneWidget);
      await tester.tap(handle);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('threads')));
      await tester.pumpAndSettle();
      expect(panel, findsOneWidget);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pumpAndSettle();
      expect(handle, findsNothing);
      expect(notice.shown, '${rig.aid} Runs');
    });

    testWidgets('the notice names the app while its screen is in front, and '
        'is removed as the screen leaves', (tester) async {
      final notice = FakeAppNotice();
      final rig = await pumpScreen(tester, notice: notice);
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(notice.shown, '${rig.aid} Runs');

      // To the background, and back.
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      await tester.pumpAndSettle();
      expect(notice.shown, isNull);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      await tester.pumpAndSettle();
      expect(
        notice.shown,
        '${rig.aid} Runs',
        reason: 'inactive is on screen (the shade pulled down)',
      );
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);

      // Another screen over it, and back.
      final nav = tester.state<NavigatorState>(find.byType(Navigator));
      unawaited(
        nav.push(MaterialPageRoute<void>(builder: (_) => const Text('over'))),
      );
      await tester.pumpAndSettle();
      expect(notice.shown, isNull);
      nav.pop();
      await tester.pumpAndSettle();
      expect(notice.shown, '${rig.aid} Runs');

      // Leaving the screen.
      await tester.pumpWidget(const SizedBox());
      expect(notice.shown, isNull);
    });

    testWidgets(
      'Comment turns comment mode on with the bar under the page; ✕ ends it',
      (tester) async {
        final rig = await pumpScreen(tester, notice: FakeAppNotice());
        rig.surface.push(rig.ready());
        await tester.pump();
        await comment(tester, rig);
        expect(modes(rig), [enter]);
        expect(bar, findsOneWidget);
        expect(panel, findsNothing);
        // The page sits above the bar, so nothing on it is covered.
        final page = tester.getRect(find.byKey(const ValueKey('fake-surface')));
        final b = tester.getRect(bar);
        expect(page.bottom, closeTo(b.top, 0.01));
        expect(b.bottom, 780);

        // The shell confirms; nothing is turned off. Comment again keeps it.
        rig.surface.push(_Rig.noDraft);
        await comment(tester, rig);
        expect(modes(rig), [enter]);

        await tester.tap(find.byKey(const ValueKey('comment-bar-close')));
        await tester.pumpAndSettle();
        expect(modes(rig), [enter, exit]);
        expect(bar, findsNothing, reason: 'nothing stays over the app');
      },
    );

    testWidgets('a slow page: the bar says Waiting for the page… from '
        'pageWait until it ends, and the page is quiet in comment mode', (
      tester,
    ) async {
      final rig = await pumpScreen(tester, notice: FakeAppNotice());
      rig.surface.push(rig.ready());
      await tester.pump();
      bool quiet() => ClaxQuiet.of(
        tester.element(find.byKey(const ValueKey('fake-surface'))),
      );
      expect(quiet(), isFalse);
      await comment(tester, rig);
      expect(quiet(), isTrue);
      final waiting = find.text('Waiting for the page…');
      // The test font is wide: the bar may take its short form.
      final tap = find.textContaining(RegExp(r'^Tap .*to comment'));
      expect(tap, findsOneWidget);
      rig.surface.push(const PageWaitMsg(waiting: true));
      await tester.pump();
      expect(waiting, findsOneWidget);
      expect(tap, findsNothing);
      rig.surface.push(const PageWaitMsg(waiting: false));
      await tester.pump();
      expect(waiting, findsNothing);
      expect(tap, findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('comment-bar-close')));
      await tester.pumpAndSettle();
      expect(quiet(), isFalse);
    });

    testWidgets('Comment while the panel is open puts the bar in its place, '
        'still commenting', (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      await comment(tester, rig);
      expect(panel, findsNothing);
      expect(bar, findsOneWidget);
      expect(modes(rig), [enter]);
    });

    testWidgets('an action the screen opened with applies once it is up', (
      tester,
    ) async {
      final rig = await pumpScreen(tester, initialAction: AppAction.threads);
      await tester.pump();
      await tester.pump();
      expect(panel, findsOneWidget);
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(modes(rig).last, enter, reason: 'the ready shell is told');
    });

    testWidgets(
      'Comment on the whole app composes on the whole app and dictates; after it the bar is back',
      (tester) async {
        final rig = await pumpScreen(tester);
        rig.surface.push(rig.ready());
        await tester.pump();
        await comment(tester, rig);
        await tester.tap(find.byKey(const ValueKey('comment-whole-app')));
        await tester.pump();
        expect(rig.surface.js.last, 'window.claxHost?.composeArtifact()');

        rig.surface.push(
          const StateMsg(
            commenting: false,
            draft: Draft(
              key: 'w1',
              kind: 'artifact',
              label: 'The whole app',
              text: '',
              clip: DraftClip.none,
            ),
            busy: false,
          ),
        );
        await tester.pumpAndSettle();
        expect(rig.voice.listens, 1, reason: 'Dictate comments is on');
        expect(find.byKey(const ValueKey('snippet-whole-app')), findsOneWidget);
        expect(bar, findsNothing, reason: "the peek's controls take over");

        // The composer's Send.
        rig.surface.push(const Sent('t9'));
        rig.surface.push(_Rig.noDraft);
        // Working spins on, so the frames are counted out.
        await frames(tester);
        expect(panel, findsNothing, reason: 'the pick opened it');
        expect(bar, findsOneWidget);
        expect(modes(rig), [enter], reason: 'comment mode stays on');
      },
    );

    testWidgets(
      'without Dictate comments the whole-app composer waits for the mic',
      (tester) async {
        final rig = await pumpScreen(
          tester,
          settings: AppSettings(dictateComments: false),
        );
        rig.surface.push(rig.ready());
        await tester.pump();
        await comment(tester, rig);
        await tester.tap(find.byKey(const ValueKey('comment-whole-app')));
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
        expect(rig.voice.listens, 0);
      },
    );

    testWidgets("the bar's Voice turns dictation on a pick off and on, and "
        'reports it', (tester) async {
      final changes = <bool>[];
      final rig = await pumpScreen(tester, onDictateChanged: changes.add);
      rig.surface.push(rig.ready());
      await tester.pump();
      await comment(tester, rig);
      final voice = find.byKey(const ValueKey('comment-voice'));
      expect(
        find.descendant(of: voice, matching: find.byIcon(Icons.mic)),
        findsOneWidget,
      );
      await tester.tap(voice);
      await tester.pumpAndSettle();
      expect(changes, [false]);
      expect(
        find.descendant(of: voice, matching: find.byIcon(Icons.mic_off)),
        findsOneWidget,
      );
      rig.surface.push(_Rig.draft);
      await tester.pumpAndSettle();
      expect(rig.voice.listens, 0, reason: 'dictation is off');
      rig.surface.push(_Rig.noDraft);
      await tester.pumpAndSettle();
      await tester.tap(voice);
      await tester.pumpAndSettle();
      expect(changes, [false, true]);
      rig.surface.push(_Rig.draft);
      await tester.pumpAndSettle();
      expect(rig.voice.listens, 1);
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
        await comment(tester, rig);
        // Labelled, or an icon named by its tooltip where the label does
        // not fit (the test font's glyphs are a full em wide).
        expect(
          find.text('Threads (1)').evaluate().length +
              find.byTooltip('Threads (1)').evaluate().length,
          1,
        );
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
      await comment(tester, rig);
      rig.surface.push(
        const StateMsg(commenting: false, draft: null, busy: false),
      );
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(modes(rig), [enter, enter]);
    });

    testWidgets('back with the bar up ends comment mode and keeps the screen', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await comment(tester, rig);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byType(AppScreen), findsOneWidget);
      expect(bar, findsNothing);
      expect(modes(rig), [enter, exit]);
    });

    testWidgets('back closes the panel opened from the bar, then the bar', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await comment(tester, rig);
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

    testWidgets('Escape and the menu commands start and end comment mode', (
      tester,
    ) async {
      final cmd = CommentsCommand();
      addTearDown(cmd.dispose);
      final rig = await pumpScreen(
        tester,
        size: const Size(1200, 800),
        comments: cmd,
      );
      rig.surface.push(rig.ready());
      await tester.pump();
      cmd.comment!();
      await tester.pumpAndSettle();
      expect(bar, findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(bar, findsNothing);
      expect(modes(rig), [enter, exit]);

      cmd.comment!();
      await tester.pumpAndSettle();
      cmd.comment!();
      await tester.pumpAndSettle();
      expect(bar, findsNothing, reason: 'Comment again ends the mode');
      expect(modes(rig), [enter, exit, enter, exit]);

      cmd.comment!();
      await tester.pumpAndSettle();
      cmd.toggle!();
      await tester.pumpAndSettle();
      expect(bar, findsNothing);
      expect(panel, findsNothing, reason: 'it ends the mode, opens nothing');
      // With nothing up, Threads opens the panel as before.
      cmd.toggle!();
      await tester.pumpAndSettle();
      expect(panel, findsOneWidget);
    });

    testWidgets(
      "a pick from the bar puts the app's composer in its place: the page keeps its height",
      (tester) async {
        final rig = await pumpScreen(tester);
        rig.surface.push(rig.ready());
        await tester.pump();
        await comment(tester, rig);
        final page = find.byKey(const ValueKey('fake-surface'));
        final withBar = tester.getRect(page);
        rig.surface.push(_Rig.draft);
        await tester.pumpAndSettle();
        expect(find.byKey(const ValueKey('composer-sheet')), findsOneWidget);
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
    });

    testWidgets(
      'a reply that lands shows a brief toast; View opens its thread',
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
              comments: 2,
              version: 3,
            ),
          ]),
        );
        await tester.pump();
        rig.worker.add(WorkerWorking(rig.aid, const ['t1']));
        await tester.pump();
        rig.worker.add(WorkerReplied(rig.aid));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 500));
        expect(find.text('Clax replied'), findsOneWidget);
        expect(
          find.text('Answered in the thread'),
          findsNothing,
          reason: 'the chip ends with its change',
        );
        await tester.tap(find.text('View'));
        await tester.pumpAndSettle();
        expect(find.byType(ThreadDetail), findsOneWidget);
        expect(
          tester.widget<ThreadDetail>(find.byType(ThreadDetail)).tid,
          't1',
        );
      },
    );

    testWidgets('the toast shows at the top and fades on its own', (
      tester,
    ) async {
      final clock = _Clock();
      final rig = await pumpScreen(tester, clock: clock);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.worker.add(WorkerReplied(rig.aid));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));
      expect(find.text('Clax replied'), findsOneWidget);
      // Under the 24 dp status bar, 16 dp from the sides: the page's own
      // navigation at the bottom stays clear.
      final card = tester.getRect(find.byKey(const ValueKey('app-toast')));
      expect(card.top, 24 + 8);
      expect(card.left, 16);
      expect(card.right, 360 - 16);
      clock.lapse();
      await tester.pumpAndSettle();
      expect(find.text('Clax replied'), findsNothing);
    });

    testWidgets('a question that lands shows a toast; View opens the '
        'questions', (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      rig.surface.push(const QuestionsMsg(1));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));
      expect(
        find.text('Clax has a question'),
        findsNothing,
        reason: 'one already open as the app opened did not land now',
      );
      rig.surface.push(const QuestionsMsg(2));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));
      expect(find.text('Clax has a question'), findsOneWidget);
      await tester.tap(find.text('View'));
      await tester.pump();
      expect(
        rig.api.calls.map((c) => c.name),
        contains('questions'),
        reason: 'the native sheet reads them from the daemon',
      );
      expect(rig.surface.js, isNot(contains(const OpenQuestions().toJs())));
    });

    testWidgets('a question that lands taps lightly', (tester) async {
      final haptics = <Object?>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'HapticFeedback.vibrate') {
            haptics.add(call.arguments);
          }
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      rig.surface.push(const QuestionsMsg(0));
      await tester.pump();
      expect(haptics, isEmpty);
      rig.surface.push(const QuestionsMsg(1));
      await tester.pump();
      expect(haptics, ['HapticFeedbackType.lightImpact']);
      rig.surface.push(const QuestionsMsg(0));
      await tester.pump();
      expect(haptics, hasLength(1), reason: 'an answer is no news');
    });

    testWidgets('back closes the questions sheet first, then the bar', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      await comment(tester, rig);
      rig.surface.push(const QuestionsMsg(1, showing: true));
      await tester.pump();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(rig.surface.js.last, const CloseQuestions().toJs());
      expect(bar, findsOneWidget, reason: 'the bar stays under the sheet');
      expect(modes(rig), [enter]);
      rig.surface.push(const QuestionsMsg(1));
      await tester.pump();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(bar, findsNothing);
      expect(find.byType(AppScreen), findsOneWidget);
      expect(modes(rig), [enter, exit]);
    });

    testWidgets('without comment mode, back closes the questions sheet and '
        'keeps the app', (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      rig.surface.push(const QuestionsMsg(1, showing: true));
      await tester.pump();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byType(AppScreen), findsOneWidget);
      expect(rig.surface.js.last, const CloseQuestions().toJs());
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
    'on macOS, View › Show Threads (⌘K) toggles the panel and Escape closes '
    'it; View › Comment (⇧⌘K) toggles comment mode',
    (tester) async {
      final enabled = <int, bool>{};
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        MacMenu.channel,
        (call) async {
          if (call.method == 'setEnabled') {
            final args = call.arguments as Map<Object?, Object?>;
            enabled[args['tag']! as int] = args['enabled']! as bool;
          }
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          MacMenu.channel,
          null,
        ),
      );
      final rig = await pumpScreen(tester, size: const Size(1200, 800));
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();

      expect(enabled[ClaxCommand.toggleThreads.tag], isTrue);
      expect(enabled[ClaxCommand.comment.tag], isTrue);
      Future<void> select([ClaxCommand c = ClaxCommand.toggleThreads]) async {
        await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
          MacMenu.channel.name,
          MacMenu.channel.codec.encodeMethodCall(MethodCall('command', c.tag)),
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
      await select(ClaxCommand.comment);
      expect(find.byKey(const ValueKey('comment-bar')), findsOneWidget);
      await select(ClaxCommand.comment);
      expect(find.byKey(const ValueKey('comment-bar')), findsNothing);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );
  testWidgets(
    'on macOS the window is told Escape is wanted while comment mode or the '
    'panel is up, and an Escape it catches from the page closes them',
    (tester) async {
      final wanted = <bool>[];
      final m = tester.binding.defaultBinaryMessenger;
      m.setMockMethodCallHandler(MacWindow.channel, (call) async {
        if (call.method == 'setEscapeWanted') {
          final args = call.arguments as Map<Object?, Object?>;
          expect(args['view'], tester.view.viewId);
          wanted.add(args['on']! as bool);
        }
        return null;
      });
      addTearDown(() => m.setMockMethodCallHandler(MacWindow.channel, null));
      final rig = await pumpScreen(tester, size: const Size(1200, 800));
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(wanted, isEmpty);

      Future<void> command(ClaxCommand c) async {
        await m.handlePlatformMessage(
          MacMenu.channel.name,
          MacMenu.channel.codec.encodeMethodCall(MethodCall('command', c.tag)),
          (_) {},
        );
        await tester.pumpAndSettle();
      }

      Future<Object?> escape() async {
        Object? reply;
        await m.handlePlatformMessage(
          MacWindow.channel.name,
          MacWindow.channel.codec.encodeMethodCall(
            MethodCall('escape', {'view': tester.view.viewId}),
          ),
          (data) => reply = data == null
              ? null
              : MacWindow.channel.codec.decodeEnvelope(data),
        );
        await tester.pumpAndSettle();
        return reply;
      }

      await command(ClaxCommand.comment);
      expect(find.byKey(const ValueKey('comment-bar')), findsOneWidget);
      expect(wanted, [true]);
      await command(ClaxCommand.comment);
      expect(wanted, [true, false]);

      await command(ClaxCommand.toggleThreads);
      expect(find.byKey(const ValueKey('app-panel')), findsOneWidget);
      expect(wanted, [true, false, true]);
      expect(await escape(), isTrue);
      expect(find.byKey(const ValueKey('app-panel')), findsNothing);
      expect(wanted, [true, false, true, false]);
      expect(await escape(), isFalse);
      expect(find.byType(AppScreen), findsOneWidget);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );
  group('on macOS the toolbar', () {
    late List<List<Map<Object?, Object?>>> items;
    late List<Map<Object?, Object?>> titles;

    /// The last items' IDs, with what is off-normal about each.
    List<String> shown() => [
      for (final i in items.last)
        [
          i['id'],
          if (i['enabled'] == false) 'disabled',
          if (i['on'] == true) 'on',
          if (i['badge'] case final b?) '$b',
        ].join(' '),
    ];

    void mockChannels(WidgetTester tester) {
      MacToolbar.reset();
      items = [];
      titles = [];
      final m = tester.binding.defaultBinaryMessenger;
      m.setMockMethodCallHandler(MacToolbar.channel, (call) async {
        if (call.method == 'setItems') {
          final args = call.arguments as Map<Object?, Object?>;
          expect(args['view'], tester.view.viewId);
          items.add([
            for (final i in args['items']! as List) i as Map<Object?, Object?>,
          ]);
        }
        return null;
      });
      m.setMockMethodCallHandler(MacWindow.channel, (call) async {
        if (call.method == 'setTitle') {
          titles.add(call.arguments as Map<Object?, Object?>);
        }
        return null;
      });
      addTearDown(() {
        m.setMockMethodCallHandler(MacToolbar.channel, null);
        m.setMockMethodCallHandler(MacWindow.channel, null);
      });
    }

    Future<void> click(WidgetTester tester, String id, {int? view}) async {
      final m = tester.binding.defaultBinaryMessenger;
      await m.handlePlatformMessage(
        MacToolbar.channel.name,
        MacToolbar.channel.codec.encodeMethodCall(
          MethodCall('clicked', {'view': view ?? tester.view.viewId, 'id': id}),
        ),
        (_) {},
      );
      await tester.pumpAndSettle();
    }

    testWidgets(
      'carries Back, Forward, the title, Comment, Threads and Share over the '
      'open app, and its clicks act on it',
      (tester) async {
        mockChannels(tester);
        final shared = <BundleFiles>[];
        final rig = await pumpScreen(
          tester,
          size: const Size(1200, 800),
          pushed: true,
          share: (_, files) async => shared.add(files),
        );
        rig.surface.push(rig.ready(version: 2, latest: 2));
        await tester.pumpAndSettle();
        // Back closes the app, which was opened over the launcher.
        expect(shown(), [
          'back',
          'forward disabled',
          'comment',
          'threads',
          'share',
        ]);
        expect(items.last.first['group'], 'navigation');
        expect(items.last[1]['group'], 'navigation');
        expect(titles.last, {
          'view': tester.view.viewId,
          'title': 'Runs',
          'subtitle': 'Version 2',
        });

        await click(tester, 'comment');
        expect(find.byKey(const ValueKey('comment-bar')), findsOneWidget);
        expect(shown()[2], 'comment on');
        expect(items.last[2]['tint'], isNotNull);
        await click(tester, 'comment');
        expect(find.byKey(const ValueKey('comment-bar')), findsNothing);

        await click(tester, 'threads');
        expect(find.byKey(const ValueKey('app-panel')), findsOneWidget);
        expect(shown()[3], 'threads on');
        await click(tester, 'threads');
        expect(find.byKey(const ValueKey('app-panel')), findsNothing);

        // Another window's click is that window's.
        await click(tester, 'threads', view: tester.view.viewId + 1);
        expect(find.byKey(const ValueKey('app-panel')), findsNothing);

        // Back steps back in the page's own history first.
        rig.surface.push(const HistoryMsg(back: true));
        await tester.pump();
        await click(tester, 'back');
        expect(rig.surface.hostCalls.whereType<GoBack>(), hasLength(1));
        expect(find.byType(AppScreen), findsOneWidget);

        await click(tester, 'share');
        expect(shared, hasLength(1));
        expect(shared.single, isA<ToolbarShareFiles>());
        expect((shared.single as ToolbarShareFiles).view, tester.view.viewId);

        rig.surface.push(const HistoryMsg(back: false));
        await tester.pump();
        await click(tester, 'back');
        expect(find.byType(AppScreen), findsNothing);
        expect(items.last, isEmpty, reason: 'the app screen took its items');
      },
      variant: TargetPlatformVariant.only(TargetPlatform.macOS),
    );

    testWidgets(
      'is empty while another screen covers the app, and comes back with it',
      (tester) async {
        mockChannels(tester);
        await pumpScreen(tester, size: const Size(1200, 800));
        _current.surface.push(_current.ready());
        await tester.pumpAndSettle();
        // At the root Back has nothing to do, and with no share there is no
        // Share.
        expect(shown(), ['back disabled', 'forward disabled', 'comment', 'threads']);
        final nav = tester.state<NavigatorState>(find.byType(Navigator));
        unawaited(
          nav.push(
            MaterialPageRoute<void>(builder: (_) => const Text('Inbox')),
          ),
        );
        await tester.pumpAndSettle();
        expect(items.last, isEmpty);
        nav.pop();
        await tester.pumpAndSettle();
        expect(shown(), ['back disabled', 'forward disabled', 'comment', 'threads']);
      },
      variant: TargetPlatformVariant.only(TargetPlatform.macOS),
    );

    testWidgets('is never declared off the Mac', (tester) async {
      mockChannels(tester);
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pumpAndSettle();
      expect(items, isEmpty);
    });
  });
  group('the change in progress', () {
    /// Bytes standing for a render; they do not decode, so the frame is
    /// what is checked.
    final png = Uint8List.fromList(const [0x89, 0x50, 0x4e, 0x47]);

    /// A change of [comment] sent and started; answers its thread.
    Future<String> start(
      WidgetTester tester,
      _Rig rig, [
      String comment = 'make the title bigger',
    ]) async {
      final tid = rig.api.sendComment(rig.aid, comment).threadId;
      rig.surface.push(rig.ready());
      rig.surface.push(Sent(tid));
      await tester.pump();
      return tid;
    }

    Finder sheet() => find.byKey(const ValueKey('change-sheet'));

    testWidgets('a tap on Working opens it before the first step, and it '
        'follows the change live', (tester) async {
      final rig = await pumpScreen(tester);
      final tid = await start(tester, rig);
      expect(_working, findsOneWidget);
      await tester.tap(_working);
      await frames(tester);
      expect(sheet(), findsOneWidget);
      expect(find.text('Change in progress'), findsOneWidget);
      expect(
        find.textContaining('make the title bigger', findRichText: true),
        findsOneWidget,
      );
      expect(find.byKey(ValueKey('change-thread-$tid')), findsOneWidget);

      rig.worker.add(WorkerWorking(rig.aid, [tid]));
      rig.worker.add(
        WorkerProgress(rig.aid, [tid], const Narration('Reading the title')),
      );
      await tester.pump();
      expect(find.byKey(const ValueKey('change-step-0')), findsOneWidget);
      expect(find.byKey(const ValueKey('change-render')), findsNothing);
      rig.worker.add(WorkerProgress(rig.aid, [tid], Inspected(png, 2)));
      await tester.pump();
      expect(find.byKey(const ValueKey('change-render')), findsOneWidget);
      expect(find.text('2 errors'), findsOneWidget);
    });

    testWidgets('one pill says Working and folds to the mark after 3 s; a '
        'tap opens the change; how it ended stays as a chip', (tester) async {
      final clock = _Clock();
      final rig = await pumpScreen(tester, clock: clock);
      final tid = await start(tester, rig);
      // Past the strip's slide.
      await tester.pump(const Duration(milliseconds: 300));
      final seen = find.byKey(const ValueKey('clax-pill'));
      expect(_pill(), findsOneWidget);
      expect(find.text('Working'), findsOneWidget);
      // 8 dp under the 24 dp status bar, 32 dp high, 16 dp from the edge.
      expect(tester.getRect(seen).top, 24 + 8);
      expect(tester.getRect(seen).height, 32);
      expect(tester.getRect(seen).right, 360 - 16);
      clock.lapse();
      await frames(tester);
      expect(find.text('Working'), findsNothing);
      expect(tester.getSize(seen), const Size(32, 32));
      expect(tester.getRect(seen).right, 360 - 16);

      // A step and its elapsed seconds do not open it: they are the sheet's.
      rig.worker.add(WorkerWorking(rig.aid, [tid]));
      rig.worker.add(
        WorkerProgress(rig.aid, [tid], const Narration('Reading the title')),
      );
      clock.t = clock.t.add(const Duration(seconds: 5));
      await frames(tester);
      expect(find.text('Working'), findsNothing);
      expect(find.text('Reading the title'), findsNothing);

      await tester.tap(_working);
      await frames(tester);
      expect(sheet(), findsOneWidget);
      // The phone's sheet has no close button: a tap outside closes it.
      await tester.tapAt(const Offset(5, 5));
      await frames(tester);
      expect(sheet(), findsNothing);

      rig.worker.add(WorkerFailed(rig.aid, 'it broke'));
      await tester.pump();
      expect(_pill(), findsNothing);
      expect(find.byKey(const ValueKey('chip-failed')), findsOneWidget);
      clock.lapse();
      await frames(tester);
      expect(
        find.byKey(const ValueKey('chip-failed')),
        findsOneWidget,
        reason: 'until it is dismissed',
      );
    });

    testWidgets('it opens on a step and while the new version loads', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      final tid = await start(tester, rig);
      rig.worker.add(WorkerWorking(rig.aid, [tid]));
      rig.worker.add(
        WorkerProgress(rig.aid, [tid], const Narration('Reading the title')),
      );
      await tester.pump();
      await tester.tap(_working);
      await frames(tester);
      expect(sheet(), findsOneWidget);
      // The phone's sheet has no close button: a tap outside closes it.
      await tester.tapAt(const Offset(5, 5));
      await frames(tester);
      expect(sheet(), findsNothing);

      // Landed, the New version toast takes the pill's place; once it
      // goes, the pill still says Working until the frame shows v4.
      rig.worker.add(WorkerLanded(rig.aid, 4));
      await tester.pump();
      expect(find.byKey(const ValueKey('newer-toast')), findsOneWidget);
      ClaxToast.current?.dismiss(now: true);
      await tester.pump();
      expect(_working, findsOneWidget);
      await tester.tap(_working);
      await frames(tester);
      expect(sheet(), findsOneWidget);

      // The frame of v4: it lands, says so, and closes itself.
      rig.surface.push(const Landed(4));
      await tester.pump();
      expect(find.text('Landed · v4'), findsOneWidget);
      await tester.pump(ChangeSheet.closeAfter);
      await frames(tester);
      expect(sheet(), findsNothing);
    });

    testWidgets('Stop stops the change and keeps the thread open', (
      tester,
    ) async {
      final stopped = <String>[];
      final retried = <String>[];
      final rig = await pumpScreen(
        tester,
        onStop: stopped.add,
        onRetry: (aid, tid) async {
          retried.add(tid);
          return true;
        },
      );
      final tid = await start(tester, rig);
      rig.worker.add(WorkerWorking(rig.aid, [tid]));
      await tester.pump();
      await tester.tap(_working);
      await frames(tester);
      await tester.tap(find.byKey(const ValueKey('change-stop')));
      await frames(tester);
      expect(stopped, [rig.aid]);
      expect(sheet(), findsNothing);
      expect(find.byKey(const ValueKey('chip-stopped')), findsOneWidget);
      expect(
        find.text(
          'Stopped. It waits until you run it again or add to the comment.',
        ),
        findsOneWidget,
      );
      rig.worker.add(WorkerCancelled(rig.aid));
      await tester.pump();
      expect(
        find.text(
          'Stopped. It waits until you run it again or add to the comment.',
        ),
        findsOneWidget,
      );
      final t = rig.api.threadOf(rig.aid, tid);
      expect(t.isOpen, isTrue);
      expect(t.comments.map((c) => c.body), ['make the title bigger']);
      await tester.tap(find.text('Run again'));
      await tester.pump();
      expect(retried, [tid]);
    });

    testWidgets('with the queue, Stop is a Cancel of the change\'s job, and '
        'a job waiting for Claude says so on the chip', (tester) async {
      final q = FakeQueueClient();
      final rig = await pumpScreen(tester, queue: q);
      final tid = await start(tester, rig);
      rig.worker.add(WorkerWorking(rig.aid, [tid]));
      q.push([changeJob('c1', rig.aid, waitingFor: WaitingFor.claude)]);
      await tester.pump();
      final waiting = _pill(ClaxPillState.waiting);
      expect(waiting, findsOneWidget);
      await tester.tap(waiting);
      await frames(tester);
      expect(find.text('Waiting for Claude'), findsWidgets);
      await tester.tap(find.byKey(const ValueKey('change-stop')));
      await frames(tester);
      expect(q.sent, [const Cancel('c1')]);
      expect(find.byKey(const ValueKey('chip-stopped')), findsOneWidget);
    });

    testWidgets('without a stop hook it offers no Stop', (tester) async {
      final rig = await pumpScreen(tester);
      await start(tester, rig);
      await tester.tap(_working);
      await frames(tester);
      expect(find.byKey(const ValueKey('change-stop')), findsNothing);
    });

    testWidgets('a change waiting for Claude says so on the pill, its '
        'sheet and its thread page, whose Cancel stops that thread alone, '
        'which then reads Cancelled', (tester) async {
      final stopped = <String>[];
      final rig = await pumpScreen(
        tester,
        onCancelThread: (aid, tid) async {
          stopped.add('$aid/$tid');
          return true;
        },
        isStopped: (tid) => stopped.any((s) => s.endsWith('/$tid')),
      );
      final tid = await start(tester, rig);
      rig.worker
        ..add(WorkerWorking(rig.aid, [tid]))
        ..add(WorkerProgress(rig.aid, [tid], const WaitingForConnection()));
      await tester.pump();
      final waiting = _pill(ClaxPillState.waiting);
      expect(waiting, findsOneWidget);
      expect(_working, findsNothing);
      expect(find.text('Waiting for Claude'), findsOneWidget);
      await tester.tap(waiting);
      await frames(tester);
      expect(find.text('Waiting for Claude'), findsWidgets);
      await tester.tap(find.byKey(const ValueKey('change-open-thread')));
      await frames(tester);
      expect(
        find.byKey(const ValueKey('thread-status-offline')),
        findsOneWidget,
      );
      await tester.tap(find.byKey(const ValueKey('thread-cancel')));
      await frames(tester);
      expect(stopped, ['${rig.aid}/$tid']);
      expect(find.byKey(const ValueKey('thread-status-offline')), findsNothing);
      expect(find.text('Cancelled'), findsOneWidget);
      expect(find.text('Clax is working on it'), findsNothing);
    });

    testWidgets('Open thread opens the thread page', (tester) async {
      final rig = await pumpScreen(tester);
      final tid = await start(tester, rig);
      await tester.tap(_working);
      await frames(tester);
      await tester.tap(find.byKey(const ValueKey('change-open-thread')));
      await frames(tester);
      expect(sheet(), findsNothing);
      expect(tester.widget<ThreadDetail>(find.byType(ThreadDetail)).tid, tid);
    });

    testWidgets('the shell outlines the thread while the sheet or the thread '
        'page shows it, and not after', (tester) async {
      final rig = await pumpScreen(tester);
      final tid = await start(tester, rig);
      List<String> focus() => [
        for (final j in rig.surface.js)
          if (j.contains('focusThread')) j,
      ];
      await tester.tap(_working);
      await frames(tester);
      expect(focus(), [FocusThread(tid).toJs()]);
      await tester.tap(find.byKey(const ValueKey('change-open-thread')));
      await frames(tester);
      expect(focus(), [
        FocusThread(tid).toJs(),
        FocusThread(tid).toJs(),
      ], reason: 'the sheet closing does not clear the page\'s claim');
      await tester.pageBack();
      await frames(tester);
      expect(focus().last, const FocusThread(null).toJs());
      expect(focus(), hasLength(3));
    });

    testWidgets('a waiting question is offered; Answer opens the questions', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      await start(tester, rig);
      rig.surface.push(const QuestionsMsg(1));
      await tester.pump();
      await tester.tap(_working);
      await frames(tester);
      expect(find.text('Clax has a question for you'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('change-answer')));
      await frames(tester);
      expect(sheet(), findsNothing);
      expect(rig.api.calls.map((c) => c.name), contains('questions'));
      expect(rig.surface.js, isNot(contains(const OpenQuestions().toJs())));
    });

    testWidgets('a reply while it is open stays, with the way to the reply', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      final tid = await start(tester, rig);
      rig.worker.add(WorkerWorking(rig.aid, [tid]));
      await tester.pump();
      await tester.tap(_working);
      await frames(tester);
      await rig.api.replyAsAgent(rig.aid, tid, 'Which title?');
      rig.worker.add(WorkerReplied(rig.aid, [tid]));
      await tester.pump();
      expect(find.byKey(const ValueKey('change-title')), findsOneWidget);
      expect(
        tester.widget<Text>(find.byKey(const ValueKey('change-title'))).data,
        'Clax replied',
      );
      await tester.tap(find.text('See the reply'));
      await frames(tester);
      final page = tester.widget<ThreadDetail>(find.byType(ThreadDetail));
      expect(page.tid, tid);
      expect(page.focusLatestReply, isTrue);
    });
  });

  group('landing on a reply', () {
    testWidgets('View on a reply to several threads opens the first at the '
        'reply, stepping through the others', (tester) async {
      final rig = await pumpScreen(tester);
      final a = rig.api.sendComment(rig.aid, 'one').threadId;
      final b = rig.api.sendComment(rig.aid, 'two').threadId;
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.worker.add(WorkerWorking(rig.aid, [a, b]));
      rig.worker.add(WorkerReplied(rig.aid, [a, b]));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));
      await tester.tap(find.text('View'));
      await frames(tester);
      var page = tester.widget<ThreadDetail>(find.byType(ThreadDetail));
      expect(page.tid, a);
      expect(page.focusLatestReply, isTrue);
      expect(page.run, [a, b]);
      expect(find.text('Run 1 of 2'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('thread-run-next')));
      await tester.pumpAndSettle();
      page = tester.widget<ThreadDetail>(find.byType(ThreadDetail));
      expect(page.tid, b);
      expect(page.focusLatestReply, isTrue);
    });

    testWidgets('initialThread opens the thread page at its comment once '
        'the page is up', (tester) async {
      final (api, aid) = await _seeded();
      final tid = api.sendComment(aid, 'bigger').threadId;
      await api.replyAsAgent(aid, tid, 'Done.');
      final reply = api.threadOf(aid, tid).comments.last.id;
      final again = await pumpScreen(
        tester,
        seed: () async => (api, aid),
        initialThread: tid,
        initialComment: reply,
      );
      expect(find.byType(ThreadDetail), findsNothing, reason: 'not yet up');
      again.surface.push(again.ready());
      await frames(tester);
      final page = tester.widget<ThreadDetail>(find.byType(ThreadDetail));
      expect(page.tid, tid);
      expect(page.focusComment, reply);
      again.surface.push(again.ready());
      await frames(tester);
      expect(find.byType(ThreadDetail), findsOneWidget, reason: 'once');
    });
  });

  group('a new version is swapped in with a tap', () {
    Finder toast() => find.byKey(const ValueKey('newer-toast'));
    bool reloaded(_Rig rig) =>
        rig.surface.hostCalls.any((c) => c is ReloadLatest);

    testWidgets('the shell’s offer shows New version ready; Reload loads it, '
        'and its frame ends the offer', (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.push(const NewerMsg(4));
      await tester.pump();
      expect(toast(), findsOneWidget);
      expect(find.text('New version ready'), findsOneWidget);
      expect(reloaded(rig), isFalse, reason: 'never without the tap');
      await tester.pump(const Duration(milliseconds: 500));
      await tester.tap(find.text('Reload'));
      await tester.pump();
      expect(reloaded(rig), isTrue);
      rig.surface.push(rig.ready(version: 4, latest: 4));
      await tester.pumpAndSettle();
      expect(toast(), findsNothing);
      // An offer of a version the page shows already is none.
      rig.surface.push(const NewerMsg(4));
      await tester.pumpAndSettle();
      expect(toast(), findsNothing);
    });

    testWidgets('the toast waits while a composer is open and shows once it '
        'closes', (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.push(_Rig.draft);
      await tester.pump();
      rig.surface.push(const NewerMsg(4));
      await tester.pump();
      expect(toast(), findsNothing);
      rig.surface.push(_Rig.noDraft);
      await tester.pump();
      await tester.pump();
      expect(toast(), findsOneWidget);
      // A composer opening over a showing toast puts it away until it closes.
      rig.surface.push(_Rig.draft);
      await tester.pumpAndSettle();
      expect(toast(), findsNothing);
      rig.surface.push(_Rig.noDraft);
      await tester.pump();
      await tester.pump();
      expect(toast(), findsOneWidget);
    });

    testWidgets('a change the worker landed ends its chip once the shell '
        'offers its version, timed to then', (tester) async {
      final rig = await pumpScreen(tester);
      final tid = rig.api.sendComment(rig.aid, 'bigger').threadId;
      rig.surface.push(rig.ready());
      rig.surface.push(Sent(tid));
      rig.worker.add(WorkerWorking(rig.aid, [tid]));
      await tester.pump();
      rig.worker.add(WorkerLanded(rig.aid, 4));
      await tester.pump();
      expect(rig.timings.log, ['start $longpressLanded']);
      rig.surface.push(const NewerMsg(4));
      await tester.pump();
      expect(_working, findsNothing);
      expect(rig.timings.log, [
        'start $longpressLanded',
        'stop $longpressLanded',
      ]);
      expect(rig.timings.extras.last['version'], 4);
      expect(toast(), findsOneWidget);
    });
  });

  group('Undo and Restore wait for their own reload', () {
    final toast = find.byKey(const ValueKey('newer-toast'));

    testWidgets('the shell’s newer for an Undo shows no New version until '
        'the reload lands, which ends it', (tester) async {
      final clock = _Clock();
      final rig = await pumpScreen(tester, clock: clock);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      await tester.tap(find.text('Versions'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('undo')));
      // The restored version is offered before the reload's frame.
      rig.surface.push(const NewerMsg(4));
      await frames(tester);
      expect(rig.surface.js.last, 'window.claxHost?.reloadLatest()');
      expect(toast, findsNothing);
      expect(find.text('New version ready'), findsNothing);
      expect(_pill(ClaxPillState.newVersion), findsNothing);
      // The reload's frame.
      rig.surface.push(const Landed(4));
      await frames(tester);
      expect(toast, findsNothing);
      expect(_pill(ClaxPillState.newVersion), findsNothing);
      // A later version is offered again.
      rig.surface.push(const NewerMsg(5));
      await frames(tester);
      expect(toast, findsOneWidget);
    });

    testWidgets('a Restore that fails offers what the shell offered', (
      tester,
    ) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await openPanel(tester);
      await tester.tap(find.text('Versions'));
      await tester.pumpAndSettle();
      // Another client publishes meanwhile, and the shell offers it; the
      // Undo puts the offer away and loses the race.
      await rig.api.publish(
        rig.aid,
        PublishRequest(ifVersion: 3, files: _page),
      );
      rig.surface.push(const NewerMsg(4));
      await tester.pump();
      expect(toast, findsOneWidget);
      rig.api.failNext(
        'restoreVersion',
        const ClaxApiError(status: 409, code: 'conflict', message: 'stale'),
      );
      await tester.tap(find.byKey(const ValueKey('undo')));
      await frames(tester);
      expect(find.text('The app changed meanwhile. Try again.'), findsOneWidget);
      // That toast goes; the version is offered.
      ClaxToast.current?.dismiss(now: true);
      await frames(tester);
      expect(toast, findsOneWidget);
    });
  });

  group('Rally of 10', () {
    final rally = find.text('Version 10. A rally!');

    testWidgets('an app’s first version 10 plays the rally once, and it is '
        'kept', (tester) async {
      final kept = <String>[];
      final settings = AppSettings();
      final clock = _Clock();
      final rig = await pumpScreen(
        tester,
        settings: settings,
        clock: clock,
        onRallied: (aid) => kept.add(aid),
      );
      rig.surface.push(rig.ready(version: 9, latest: 9));
      await tester.pump();
      rig.surface.push(const Landed(10));
      await frames(tester);
      expect(rally, findsOneWidget);
      final mark = tester.widget<ClaxMark>(
        find.byKey(const ValueKey('rally-mark')),
      );
      expect(mark.state, ClaxMarkState.rally);
      expect(kept, [rig.aid]);
      clock.lapse();
      await tester.pumpAndSettle();
      expect(rally, findsNothing);
      // Again at 10 (a reload, an Undo and back): nothing.
      rig.surface.push(const Landed(10));
      await frames(tester);
      expect(rally, findsNothing);
      expect(kept, [rig.aid]);
    });

    testWidgets('version 10 offered by the shell rallies in its New version '
        'toast, with Reload', (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready(version: 9, latest: 9));
      await tester.pump();
      rig.surface.push(const NewerMsg(10));
      await tester.pump();
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('newer-toast')),
          matching: rally,
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Reload'));
      await tester.pump();
      rig.surface.push(const Landed(10));
      await frames(tester);
      expect(rally, findsNothing, reason: 'once');
    });

    testWidgets('an app that rallied before, or a version that is not 10, '
        'says New version ready', (tester) async {
      final settings = AppSettings();
      final again = await pumpScreen(tester, settings: settings);
      settings.rallied.add(again.aid);
      again.surface.push(again.ready(version: 9, latest: 9));
      await tester.pump();
      again.surface.push(const NewerMsg(10));
      await tester.pump();
      expect(rally, findsNothing);
      expect(find.text('New version ready'), findsOneWidget);
      expect(find.byKey(const ValueKey('landed-mark')), findsOneWidget);
    });

    testWidgets('each app has its own rally', (tester) async {
      final settings = AppSettings();
      final kept = <String>[];
      void keep(String aid) {
        kept.add(aid);
        settings.rallied = [...settings.rallied, aid];
      }

      final one = await pumpScreen(tester, settings: settings, onRallied: keep);
      one.surface.push(one.ready(version: 9, latest: 9));
      one.surface.push(const Landed(10));
      await frames(tester);
      expect(rally, findsOneWidget);
      // The screen goes, and its toast with it.
      await tester.pumpWidget(const SizedBox());
      expect(rally, findsNothing);

      final two = await pumpScreen(
        tester,
        settings: settings,
        onRallied: keep,
        seed: () async => (one.api, one.api.seedArtifact('Groceries', _page)),
      );
      two.surface.push(two.ready(version: 9, latest: 9));
      two.surface.push(const Landed(10));
      await frames(tester);
      expect(rally, findsOneWidget);
      expect(kept, [one.aid, two.aid]);
      await tester.pumpWidget(const SizedBox());

      final first = await pumpScreen(
        tester,
        settings: settings,
        onRallied: keep,
        seed: () async => (one.api, one.aid),
      );
      first.surface.push(first.ready(version: 9, latest: 9));
      first.surface.push(const Landed(10));
      await frames(tester);
      expect(rally, findsNothing);
      expect(kept, [one.aid, two.aid]);
    });

    testWidgets('each hit of the rally is a light tick, none in comment '
        'mode', (tester) async {
      final ticks = <Object?>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'HapticFeedback.vibrate') ticks.add(call.arguments);
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      final clock = _Clock();
      final rig = await pumpScreen(tester, clock: clock);
      rig.surface.push(rig.ready(version: 9, latest: 9));
      rig.surface.push(const Landed(10));
      await frames(tester);
      expect(ticks, isEmpty, reason: 'on the clock');
      clock.lapse();
      await tester.pump();
      expect(ticks, List.filled(10, 'HapticFeedbackType.selectionClick'));
    });
  });

  group('the New version toast without the stream', () {
    Finder toast() => find.byKey(const ValueKey('newer-toast'));

    testWidgets('the worker landing a version offers it with no newer from '
        'the shell, and Reload loads it; the shell\'s later newer for it is '
        'not offered again', (tester) async {
      final rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.worker.add(WorkerLanded(rig.aid, 4));
      await tester.pump();
      expect(toast(), findsOneWidget);
      await tester.pump(const Duration(milliseconds: 500));
      await tester.tap(find.text('Reload'));
      await tester.pump();
      expect(rig.surface.hostCalls.whereType<ReloadLatest>(), hasLength(1));
      // It fades; the stream's newer for the same version comes late.
      await tester.pump(const Duration(seconds: 10));
      await tester.pumpAndSettle();
      expect(toast(), findsNothing);
      rig.surface.push(const NewerMsg(4));
      await tester.pumpAndSettle();
      expect(toast(), findsNothing);
    });

    testWidgets('leaving the app screen takes the toast with it', (
      tester,
    ) async {
      final rig = await pumpScreen(tester, pushed: true);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.push(const NewerMsg(4));
      await tester.pump();
      expect(toast(), findsOneWidget);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.text('Launcher'), findsOneWidget);
      expect(toast(), findsNothing);
    });

    testWidgets('leaving the app screen puts away only the toast, not one '
        'queued behind it', (tester) async {
      final rig = await pumpScreen(tester, pushed: true);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.push(const NewerMsg(4));
      await tester.pump();
      expect(toast(), findsOneWidget);
      tester
          .state<ScaffoldMessengerState>(find.byType(ScaffoldMessenger))
          .showSnackBar(const SnackBar(content: Text('Saved elsewhere')));
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.text('Launcher'), findsOneWidget);
      expect(toast(), findsNothing);
      expect(find.text('Saved elsewhere'), findsOneWidget);
    });

    testWidgets('leaving the app screen leaves a toast that took the '
        'toast\'s place, and the toast queued behind it never shows', (
      tester,
    ) async {
      final rig = await pumpScreen(tester, pushed: true);
      rig.surface.push(rig.ready());
      await tester.pump();
      rig.surface.push(const NewerMsg(4));
      await tester.pump();
      expect(toast(), findsOneWidget);
      tester.state<ScaffoldMessengerState>(find.byType(ScaffoldMessenger))
        ..hideCurrentSnackBar()
        ..showSnackBar(
          const SnackBar(
            content: Text('Saved elsewhere'),
            duration: Duration(seconds: 2),
          ),
        );
      // The toast waits behind it.
      await tester.pumpAndSettle(const Duration(milliseconds: 100));
      expect(find.text('Saved elsewhere'), findsOneWidget);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.text('Launcher'), findsOneWidget);
      expect(find.text('Saved elsewhere'), findsOneWidget);
      // It fades; the toast that waited behind it does not come up.
      await tester.pump(const Duration(seconds: 3));
      await tester.pumpAndSettle();
      expect(find.text('Saved elsewhere'), findsNothing);
      expect(toast(), findsNothing);
    });
  });

  group('back goes back in the app first', () {
    Future<_Rig> pushed(WidgetTester tester) async {
      final rig = await pumpScreen(tester, pushed: true);
      rig.surface.push(rig.ready());
      await tester.pump();
      return rig;
    }

    testWidgets('back goes back in the page’s own history while it has one, '
        'then leaves the app', (tester) async {
      final rig = await pushed(tester);
      rig.surface.push(const HistoryMsg(back: true));
      await tester.pump();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byType(AppScreen), findsOneWidget);
      expect(rig.surface.hostCalls.whereType<GoBack>(), hasLength(1));
      rig.surface.push(const HistoryMsg(back: false));
      await tester.pump();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byType(AppScreen), findsNothing);
      expect(find.text('Launcher'), findsOneWidget);
      expect(rig.surface.hostCalls.whereType<GoBack>(), hasLength(1));
    });

    testWidgets('in a split window\'s detail, back past the page\'s history '
        'leaves through leave and the route stays', (tester) async {
      var left = 0;
      final rig = await pumpScreen(tester, leave: () => left++);
      rig.surface.push(rig.ready());
      rig.surface.push(const HistoryMsg(back: true));
      await tester.pump();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(rig.surface.hostCalls.whereType<GoBack>(), hasLength(1));
      expect(left, 0);
      rig.surface.push(const HistoryMsg(back: false));
      await tester.pump();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(left, 1);
      expect(find.byType(AppScreen), findsOneWidget);
      expect(rig.surface.hostCalls.whereType<GoBack>(), hasLength(1));
    });

    /// Sends one predictive back gesture's [method] as Android's
    /// OnBackInvokedCallback does, from the left edge.
    Future<void> gesture(
      WidgetTester tester,
      String method, [
      double progress = 0,
    ]) async {
      final args = method == 'commitBackGesture'
          ? null
          : <String, Object?>{
              'touchOffset': <double>[5 + 300 * progress, 300],
              'progress': progress,
              'swipeEdge': 0,
            };
      await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
        'flutter/backgesture',
        const StandardMethodCodec().encodeMethodCall(MethodCall(method, args)),
        (_) {},
      );
      await tester.pump();
    }

    testWidgets('a predictive back gesture inside the page’s history leaves '
        'the app in place and goes back in the page', (tester) async {
      final rig = await pushed(tester);
      rig.surface.push(const HistoryMsg(back: true));
      await tester.pump();
      final full = tester.getRect(find.byType(AppScreen));
      await gesture(tester, 'startBackGesture');
      await gesture(tester, 'updateBackGestureProgress', 0.5);
      // The route does not follow the finger: the back stays in the page.
      expect(tester.getRect(find.byType(AppScreen)), full);
      await gesture(tester, 'commitBackGesture');
      await tester.pumpAndSettle();
      expect(find.byType(AppScreen), findsOneWidget);
      expect(tester.getRect(find.byType(AppScreen)), full);
      expect(rig.surface.hostCalls.whereType<GoBack>(), hasLength(1));
    });

    testWidgets('a predictive back gesture at the page’s first entry draws '
        'the app back with the finger and leaves it', (tester) async {
      final rig = await pushed(tester);
      final full = tester.getRect(find.byType(AppScreen));
      await gesture(tester, 'startBackGesture');
      await gesture(tester, 'updateBackGestureProgress', 0.5);
      // The predictive transition shrinks the app toward the launcher.
      expect(tester.getRect(find.byType(AppScreen)), isNot(full));
      await gesture(tester, 'commitBackGesture');
      await tester.pumpAndSettle();
      expect(find.byType(AppScreen), findsNothing);
      expect(find.text('Launcher'), findsOneWidget);
      expect(rig.surface.hostCalls.whereType<GoBack>(), isEmpty);
    });

    testWidgets('back with a composer open and the panel closed cancels the '
        'draft, before the page\'s history or leaving', (tester) async {
      final rig = await pushed(tester);
      rig.surface.push(const HistoryMsg(back: true));
      rig.surface.push(_Rig.draft);
      await tester.pump();
      // The panel closes; the composer stays.
      await twoFingerSwipe(tester, 120);
      expect(find.byKey(const ValueKey('app-panel')), findsNothing);
      await tester.binding.handlePopRoute();
      await tester.pump();
      expect(rig.surface.hostCalls.whereType<CancelDraft>(), hasLength(1));
      expect(rig.surface.hostCalls.whereType<GoBack>(), isEmpty);
      expect(find.byType(AppScreen), findsOneWidget);
    });

    testWidgets('back closes the comment bar before going back in the page', (
      tester,
    ) async {
      final rig = await pushed(tester);
      rig.surface.push(const HistoryMsg(back: true));
      rig.actions.add(AppAction.comment);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('comment-bar')), findsOneWidget);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('comment-bar')), findsNothing);
      expect(rig.surface.hostCalls.whereType<GoBack>(), isEmpty);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(rig.surface.hostCalls.whereType<GoBack>(), hasLength(1));
      expect(find.byType(AppScreen), findsOneWidget);
    });
  });

  group('edge to edge', () {
    Color band(WidgetTester tester, String edge) => tester
        .widget<ColoredBox>(find.byKey(ValueKey('system-band-$edge')))
        .color;

    testWidgets('the status band takes the page\'s top colour, the '
        'navigation band its bottom, and the icons follow', (tester) async {
      final rig = await pumpScreen(tester);
      tester.view.padding = const FakeViewPadding(top: 24 * 3, bottom: 16 * 3);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(band(tester, 'top'), ClaxTokens.light.bg);
      rig.surface.push(ChromeMsg(const Color(0xFF102030), Colors.white));
      await tester.pump();
      await tester.pump();
      expect(band(tester, 'top'), const Color(0xFF102030));
      expect(band(tester, 'bottom'), Colors.white);
      expect(SystemChrome.latestStyle!.statusBarIconBrightness, Brightness.light);
      expect(
        SystemChrome.latestStyle!.systemNavigationBarIconBrightness,
        Brightness.dark,
      );
      // The page is between the bands, and the screen is its whole height.
      final page = tester.getRect(find.byKey(const ValueKey('fake-surface')));
      expect(page.top, 24);
      expect(page.bottom, 780 - 16);
    });

    testWidgets('the navigation band goes while the comment bar holds the '
        'bottom inset', (tester) async {
      final rig = await pumpScreen(tester);
      tester.view.padding = const FakeViewPadding(top: 24 * 3, bottom: 16 * 3);
      rig.surface.push(rig.ready());
      rig.surface.push(ChromeMsg(const Color(0xFF102030), Colors.white));
      await tester.pump();
      expect(find.byKey(const ValueKey('system-band-bottom')), findsOneWidget);
      rig.actions.add(AppAction.comment);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('comment-bar')), findsOneWidget);
      expect(find.byKey(const ValueKey('system-band-bottom')), findsNothing);
      expect(find.byKey(const ValueKey('system-band-top')), findsOneWidget);
    });
  });

  group('the pill at the top', () {
    // Insets in logical pixels: a short status bar, a tall one, a punch-hole
    // cutout taller than the bar, and a landscape cutout on the left.
    const insets = {
      'status bar 24': EdgeInsets.only(top: 24),
      'status bar 48': EdgeInsets.only(top: 48),
      'cutout 56': EdgeInsets.only(top: 56),
      'landscape cutout': EdgeInsets.only(top: 24, left: 44, right: 16),
    };
    for (final MapEntry(key: name, value: pad) in insets.entries) {
      testWidgets('the pill sits 8 dp under the top inset and inside the '
          'side insets, folded too: $name', (tester) async {
        final clock = _Clock();
        final landscape = pad.left > 0;
        final rig = await pumpScreen(
          tester,
          clock: clock,
          // A landscape phone narrower than medium: wider, it lays out
          // as a pane under a top app bar, with no pill.
          size: landscape ? const Size(568, 320) : const Size(360, 780),
        );
        tester.view.padding = FakeViewPadding(
          top: pad.top * 3,
          left: pad.left * 3,
          right: pad.right * 3,
        );
        final tid = rig.api.sendComment(rig.aid, 'bigger').threadId;
        rig.surface.push(rig.ready());
        rig.surface.push(Sent(tid));
        await frames(tester);
        final width = landscape ? 568.0 : 360.0;
        final page = find.byKey(const ValueKey('fake-surface'));
        for (final folded in [false, true]) {
          if (folded) {
            clock.lapse();
            await frames(tester);
          }
          final pill = tester.getRect(find.byKey(const ValueKey('clax-pill')));
          final target = tester.getRect(
            find.byKey(const ValueKey('clax-pill-target')),
          );
          expect(pill.top, pad.top + 8);
          expect(pill.height, 32);
          expect(pill.right, width - pad.right - 16);
          expect(pill.left, greaterThanOrEqualTo(pad.left));
          expect(target.height, greaterThanOrEqualTo(48));
          expect(target.width, greaterThanOrEqualTo(48));
          expect(target.top, greaterThanOrEqualTo(pad.top));
          // The pill's strip pushes the page down: nothing covers it.
          expect(tester.getRect(page).top, pad.top + PillStrip.height);
          expect(target.bottom, lessThanOrEqualTo(tester.getRect(page).top));
        }
      });
    }

    testWidgets('one pill at a time: Working before Building before New '
        'version before Location, and none while a toast is up', (
      tester,
    ) async {
      final clock = _Clock();
      final rig = await pumpScreen(tester, clock: clock);
      rig.surface.push(rig.ready());
      await tester.pump();
      expect(_pill(), findsNothing, reason: 'nothing goes on');
      rig.surface.push(const NewerMsg(4));
      await tester.pump();
      expect(find.byKey(const ValueKey('newer-toast')), findsOneWidget);
      expect(_pill(), findsNothing, reason: 'the toast says it');
      clock.lapse();
      await frames(tester);
      expect(find.byKey(const ValueKey('newer-toast')), findsNothing);
      expect(_pill(ClaxPillState.newVersion), findsOneWidget);
      expect(find.text('New version'), findsOneWidget);
      clock.lapse();
      await frames(tester);
      expect(find.text('New version'), findsNothing, reason: 'folded');
      final tid = rig.api.sendComment(rig.aid, 'bigger').threadId;
      rig.surface.push(Sent(tid));
      await tester.pump();
      expect(_pill(), findsOneWidget);
      expect(_working, findsOneWidget);
      expect(find.text('Working'), findsOneWidget, reason: 'it opens again');
    });

    testWidgets('New version’s Show loads it', (tester) async {
      final clock = _Clock();
      final rig = await pumpScreen(tester, clock: clock);
      rig.surface.push(rig.ready());
      rig.surface.push(const NewerMsg(4));
      await tester.pump();
      clock.lapse();
      await frames(tester);
      await tester.tap(_pill(ClaxPillState.newVersion));
      await tester.pump();
      expect(rig.surface.hostCalls.whereType<ReloadLatest>(), hasLength(1));
      rig.surface.push(rig.ready(version: 4, latest: 4));
      await tester.pump();
      expect(_pill(), findsNothing);
    });

    testWidgets('a change under way opens the strip: the page moves down by '
        'its height and stays there as the pill folds, and a tap on the '
        'folded mark opens the change', (tester) async {
      final clock = _Clock();
      final rig = await pumpScreen(tester, clock: clock);
      tester.view.padding = const FakeViewPadding(top: 24 * 3);
      final page = find.byKey(const ValueKey('fake-surface'));
      rig.surface.push(rig.ready());
      await frames(tester);
      expect(tester.getRect(page).top, 24);
      final tid = rig.api.sendComment(rig.aid, 'bigger').threadId;
      rig.surface.push(Sent(tid));
      await frames(tester);
      expect(_working, findsOneWidget);
      expect(tester.getRect(page).top, 24 + PillStrip.height);
      expect(tester.getRect(page).bottom, 780);
      clock.lapse();
      await frames(tester);
      expect(find.text('Working'), findsNothing, reason: 'folded');
      expect(tester.getRect(page).top, 24 + PillStrip.height);
      await tester.tap(_working);
      await frames(tester);
      expect(find.byKey(const ValueKey('change-sheet')), findsOneWidget);
    });

    testWidgets('at 200% text the pill’s words are whole', (tester) async {
      final rig = await pumpScreen(tester);
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      rig.surface.push(rig.ready());
      rig.surface.push(const Sent('thread-1'));
      await frames(tester);
      final words = find.text('Working');
      final paragraph = tester.renderObject<RenderParagraph>(words);
      expect(paragraph.didExceedMaxLines, isFalse);
      expect(
        tester.getSize(words).width,
        closeTo(paragraph.getMaxIntrinsicWidth(double.infinity), 0.5),
      );
    });
  });

  group('on wide Android an app shows in a pane', () {
    Finder key(String k) => find.byKey(ValueKey(k));
    const open = ThreadsMsg([
      ThreadSummary(
        id: 't1',
        status: 'open',
        summary: 'Bigger title',
        sent: true,
        comments: 1,
        version: 3,
      ),
    ]);

    testWidgets(
      'at 1280 a 64 dp top app bar carries Back, the title, Comment, Threads '
      'with its count, Share and More, and there is no Clax pill',
      (tester) async {
        final shared = <BundleFiles>[];
        final rig = await pumpScreen(
          tester,
          size: const Size(1280, 800),
          pushed: true,
          share: (_, files) async => shared.add(files),
        );
        rig.surface.push(rig.ready());
        rig.surface.push(open);
        rig.surface.push(const Sent('thread-1'));
        await frames(tester);
        final bar = tester.getRect(key('pane-bar'));
        expect(bar.top, 0);
        expect(bar.width, 1280);
        // Under the 24 dp status bar.
        expect(bar.height, 24 + 64);
        for (final k in [
          'pane-back',
          'pane-comment',
          'pane-threads',
          'pane-share',
          'pane-more',
        ]) {
          expect(key(k), findsOneWidget, reason: k);
          final r = tester.getRect(key(k));
          expect(r.top, greaterThanOrEqualTo(24), reason: k);
          expect(r.bottom, lessThanOrEqualTo(24 + 64), reason: k);
        }
        expect(
          find.descendant(of: key('pane-bar'), matching: find.text('Runs')),
          findsOneWidget,
        );
        expect(
          find.descendant(of: key('pane-threads'), matching: find.text('1')),
          findsOneWidget,
        );
        expect(find.byType(ClaxPill), findsNothing);
        expect(key('pane-status'), findsOneWidget);
        expect(tester.getRect(key('fake-surface')).top, 24 + 64);

        await tester.tap(key('pane-share'));
        await tester.pump();
        expect(shared, hasLength(1));
        await tester.tap(key('pane-comment'));
        await frames(tester);
        expect(find.byType(CommentBar), findsOneWidget);
        await tester.tap(key('pane-comment'));
        await frames(tester);
        expect(find.byType(CommentBar), findsNothing);
        // Back leaves the app, opened over the launcher.
        await tester.tap(key('pane-back'));
        await frames(tester);
        expect(find.text('Launcher'), findsOneWidget);
      },
    );

    testWidgets('at 700 the bar is there too; on a phone it is not', (
      tester,
    ) async {
      var rig = await pumpScreen(tester, size: const Size(700, 900));
      rig.surface.push(rig.ready());
      await frames(tester);
      expect(key('pane-bar'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      rig = await pumpScreen(tester);
      rig.surface.push(rig.ready());
      await frames(tester);
      expect(key('pane-bar'), findsNothing);
    });

    testWidgets("More offers Inspect, Versions and Star; Versions opens the "
        "panel's versions", (tester) async {
      var inspected = 0;
      final rig = await pumpScreen(
        tester,
        size: const Size(1280, 800),
        onInspect: () => inspected++,
      );
      rig.surface.push(rig.ready());
      await frames(tester);
      await tester.tap(key('pane-more'));
      await tester.pumpAndSettle();
      expect(find.text('Inspect'), findsOneWidget);
      expect(find.text('Versions'), findsOneWidget);
      await tester.tap(find.text('Inspect'));
      await tester.pumpAndSettle();
      expect(inspected, 1);
      await tester.tap(key('pane-more'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Versions'));
      await tester.pumpAndSettle();
      expect(key('app-panel'), findsOneWidget);
      expect(
        tester.widget<AppPanel>(find.byType(AppPanel)).tab,
        PanelTab.versions,
      );
    });

    testWidgets('a notice and the New version toast are inline bars under '
        'the app bar, the width of the pane', (tester) async {
      final rig = await pumpScreen(tester, size: const Size(1280, 800));
      rig.surface.push(rig.ready());
      await frames(tester);
      rig.surface.push(const NewerMsg(4));
      await tester.pumpAndSettle();
      var toast = tester.getRect(key('newer-toast'));
      expect(toast.top, 24 + 64);
      expect(toast.left, 0);
      expect(toast.width, 1280);
      rig.surface.push(const Notice('Saved'));
      await tester.pumpAndSettle();
      toast = tester.getRect(key('app-notice'));
      expect(toast.top, 24 + 64);
      expect(toast.width, 1280);
    });

    testWidgets('a change waiting for Claude says so in the top app bar, '
        'whose tap opens the change', (tester) async {
      final rig = await pumpScreen(tester, size: const Size(1280, 800));
      rig.surface.push(rig.ready());
      rig.surface.push(const Sent('thread-1'));
      await frames(tester);
      rig.worker
        ..add(WorkerWorking(rig.aid, ['thread-1']))
        ..add(
          WorkerProgress(rig.aid, ['thread-1'], const WaitingForConnection()),
        );
      await frames(tester);
      expect(
        find.descendant(
          of: key('pane-status'),
          matching: find.text('Waiting for Claude'),
        ),
        findsOneWidget,
      );
      await tester.tap(key('pane-status'));
      await frames(tester);
      expect(key('change-sheet'), findsOneWidget);
    });

    testWidgets(
      'the change opens in a side sheet, and how it ended pushes the page '
      'down rather than cover its top 64 dp',
      (tester) async {
        final rig = await pumpScreen(tester, size: const Size(1280, 800));
        rig.surface.push(rig.ready());
        rig.surface.push(const Sent('thread-1'));
        await frames(tester);
        expect(key('chip-working'), findsNothing);
        await tester.tap(key('pane-status'));
        await frames(tester);
        final sheet = tester.getRect(key('change-sheet'));
        expect(sheet.right, 1280);
        expect(sheet.left, greaterThan(1280 - 420));
        await tester.tapAt(const Offset(200, 400));
        await frames(tester);
        expect(key('change-sheet'), findsNothing);

        rig.worker.add(WorkerFailed(rig.aid, 'The API key was refused'));
        await frames(tester);
        final page = tester.getRect(key('fake-surface'));
        final chip = tester.getRect(key('chip-failed'));
        expect(chip.top, greaterThanOrEqualTo(24 + 64));
        expect(chip.bottom, lessThanOrEqualTo(page.top));
        await tester.tap(key('chip-dismiss'));
        await frames(tester);
        expect(key('pane-outcome'), findsNothing);
        expect(tester.getRect(key('fake-surface')).top, 24 + 64);
      },
    );

    testWidgets('the build is the bar\'s status, and its plan a side sheet', (
      tester,
    ) async {
      final plans = PlanStore();
      final builds = _Builds();
      final rig = await pumpScreen(
        tester,
        size: const Size(1280, 800),
        build: BuildLink(plans: plans, control: builds),
        seed: () async {
          final seeded = await _seeded();
          await plans.set(seeded.$2, const [
            PlanFeature(id: 'f1', title: 'Log a run', status: FeatureStatus.done),
            PlanFeature(id: 'f2', title: 'Weekly chart'),
          ]);
          builds.emit(BuildState(seeded.$2, BuildPhase.running, featureId: 'f2'));
          return seeded;
        },
      );
      rig.surface.push(rig.ready());
      await frames(tester);
      expect(key('chip-building'), findsNothing);
      expect(
        find.descendant(of: key('pane-status'), matching: find.textContaining('1/2')),
        findsOneWidget,
      );
      await tester.tap(key('pane-status'));
      await frames(tester);
      final sheet = tester.getRect(key('build-side-sheet'));
      expect(sheet.right, 1280);
      expect(sheet.width, 360);
      expect(key('build-sheet'), findsOneWidget);
    });

    testWidgets(
      'at 1300 the threads side sheet pushes the page by 360; at 1000 it '
      'lies over the page, and a tap beside it closes it',
      (tester) async {
        var rig = await pumpScreen(tester, size: const Size(1300, 800));
        rig.surface.push(rig.ready());
        await tester.tap(key('pane-threads'));
        await tester.pumpAndSettle();
        var panel = tester.getRect(key('app-panel'));
        expect(panel.width, 360);
        expect(panel.right, 1300);
        expect(tester.getRect(key('fake-surface')).width, 1300 - 360);
        expect(key('threads-scrim'), findsNothing);
        expect(key('threads-pane-edge'), findsNothing);

        await tester.pumpWidget(const SizedBox());
        rig = await pumpScreen(tester, size: const Size(1000, 800));
        rig.surface.push(rig.ready());
        await tester.tap(key('pane-threads'));
        await tester.pumpAndSettle();
        panel = tester.getRect(key('app-panel'));
        expect(panel.width, 360);
        expect(panel.right, 1000);
        expect(tester.getRect(key('fake-surface')).width, 1000);
        await tester.tap(key('threads-scrim'), warnIfMissed: false);
        await tester.tapAt(const Offset(200, 400));
        await tester.pumpAndSettle();
        expect(key('app-panel'), findsNothing);
      },
    );
  });
}
