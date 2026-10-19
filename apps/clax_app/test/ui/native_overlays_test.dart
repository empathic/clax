// The app screen draws the shell's notices, its draft banner and a deleted
// app natively: `notice` shows a toast whose action calls `noticeAction`,
// `draft` shows the banner whose Show draft and Back call `showDraft` and
// `hideDraft`, and `gone` shows "This app was deleted." with Back to Home.
import 'dart:async';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

/// A clock that never moves: toasts stay up for the test.
class _Still implements Clock {
  @override
  DateTime now() => DateTime.utc(2026, 10, 19);

  @override
  Future<void> delay(Duration d) => Completer<void>().future;
}

void main() {
  late FakeAppSurface surface;

  /// The app screen over a home route, its page ready; [width] in logical
  /// pixels.
  Future<void> pumpScreen(WidgetTester tester, {double width = 360}) async {
    tester.view.physicalSize = Size(width * 3, 780 * 3);
    tester.view.devicePixelRatio = 3;
    addTearDown(tester.view.reset);
    final api = FakeClaxApi();
    final aid = api.seedArtifact('Dashboard', const {
      'index.html': '<!doctype html><h1>Dashboard</h1>',
    });
    surface = FakeAppSurface();
    final worker = StreamController<WorkerEvent>.broadcast();
    addTearDown(worker.close);
    final nav = GlobalKey<NavigatorState>();
    await tester.pumpWidget(
      MaterialApp(
        navigatorKey: nav,
        theme: claxTheme(Brightness.light, TargetPlatform.android),
        home: const Scaffold(body: Text('home screen')),
      ),
    );
    unawaited(
      nav.currentState!.push(
        MaterialPageRoute<void>(
          builder: (_) => AppScreen(
            aid: aid,
            base: Uri.parse('http://127.0.0.1:7999'),
            api: api,
            surface: surface,
            voice: FakeVoice(),
            settings: AppSettings(),
            timings: NoTimings(),
            worker: worker.stream,
            clock: _Still(),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    surface.push(
      Ready(artifact: aid, version: 1, latest: 1, file: 'index.html'),
    );
    await tester.pump();
  }

  test('the app declares notices and the draft banner native', () {
    expect(nativeOverlays, containsAll(['ask', 'questions', 'notice', 'draft']));
  });

  group('notices', () {
    testWidgets('a notice is a toast at the top whose action calls '
        'noticeAction', (tester) async {
      await pumpScreen(tester);
      surface.push(const Notice('Saved', action: (id: 'n1', label: 'Undo')));
      await tester.pumpAndSettle();
      expect(find.text('Saved'), findsOneWidget);
      final card = tester.getRect(find.byKey(const ValueKey('app-notice')));
      expect(card.top, lessThan(100), reason: 'at the top, not the bottom');
      await tester.tap(find.text('Undo'));
      await tester.pumpAndSettle();
      expect(surface.hostCalls.whereType<NoticeAction>().map((c) => c.id), [
        'n1',
      ]);
      expect(find.text('Saved'), findsNothing);
    });

    testWidgets('an error notice leads with the danger icon; an info one '
        'with none', (tester) async {
      await pumpScreen(tester);
      surface.push(const Notice('Saved'));
      await tester.pumpAndSettle();
      expect(find.byKey(ClaxToast.errorIconKey), findsNothing);
      surface.push(
        const Notice(
          'Couldn’t load the latest version',
          tone: NoticeTone.error,
          action: (id: 'retry', label: 'Try again'),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('Couldn’t load the latest version'), findsOneWidget);
      expect(find.byKey(ClaxToast.errorIconKey), findsOneWidget);
      expect(find.text('Try again'), findsOneWidget);
    });

    testWidgets('from medium width a notice is a bar across the top', (
      tester,
    ) async {
      await pumpScreen(tester, width: 900);
      surface.push(const Notice('Saved'));
      await tester.pumpAndSettle();
      final bar = tester.getRect(find.byKey(const ValueKey('app-notice')));
      expect(bar.left, 0);
      expect(bar.width, 900);
    });
  });

  group('the draft banner', () {
    final banner = find.byKey(const ValueKey('draft-banner'));

    testWidgets('drafting says so, and Show draft calls showDraft', (
      tester,
    ) async {
      await pumpScreen(tester);
      surface.push(const DraftMsg(Drafting('Clax', null)));
      await tester.pump();
      expect(find.text('Clax is drafting…'), findsOneWidget);
      expect(tester.getSize(banner).height, greaterThanOrEqualTo(48));
      await tester.tap(find.text('Show draft'));
      await tester.pump();
      expect(surface.hostCalls.whereType<ShowDraft>().map((c) => c.by), [
        'Clax',
      ]);
    });

    testWidgets('showing says so, and Back calls hideDraft', (tester) async {
      await pumpScreen(tester);
      surface.push(const DraftMsg(Showing('Clax', 3, 'Bigger heading')));
      await tester.pump();
      expect(find.text('Showing Clax’s draft'), findsOneWidget);
      await tester.tap(find.text('Back'));
      await tester.pump();
      expect(surface.hostCalls.whereType<HideDraft>(), hasLength(1));
    });

    testWidgets('it sits under the status bar and pushes the page down, '
        'and a null state takes it away', (tester) async {
      await pumpScreen(tester);
      final page = find.byKey(const ValueKey('fake-surface'));
      final top = tester.getTopLeft(page).dy;
      surface.push(const DraftMsg(Drafting('Clax', null)));
      await tester.pump();
      expect(tester.getRect(banner).top, greaterThanOrEqualTo(top));
      expect(
        tester.getTopLeft(page).dy,
        greaterThanOrEqualTo(tester.getRect(banner).bottom),
        reason: 'the banner covers none of the page',
      );
      surface.push(const DraftMsg(null));
      await tester.pump();
      expect(banner, findsNothing);
      expect(tester.getTopLeft(page).dy, top);
    });
  });

  group('a deleted app', () {
    testWidgets('gone says so, and Back to Home goes home', (tester) async {
      await pumpScreen(tester);
      surface.push(const DraftMsg(Drafting('Clax', null)));
      await tester.pump();
      surface.push(const GoneMsg());
      await tester.pumpAndSettle();
      expect(find.text('This app was deleted.'), findsOneWidget);
      expect(find.byType(ClaxEmpty), findsOneWidget);
      expect(find.text('Clax is drafting…'), findsNothing);
      await tester.tap(find.text('Back to Home'));
      await tester.pumpAndSettle();
      expect(find.byType(AppScreen), findsNothing);
      expect(find.text('home screen'), findsOneWidget);
    });
  });
}
