// The app screen draws the shell's permission prompt natively: an `ask`
// message shows `AskPrompt`, its answer goes back as `askAnswer`, and
// `askCancel` takes it down without one.
import 'dart:async';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

/// A clock that moves only on [elapse], finishing the delays it reaches.
class _Clock implements Clock {
  final _waits = <(DateTime, Completer<void>)>[];
  var _now = DateTime.utc(2026, 10, 19);

  @override
  DateTime now() => _now;

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _waits.add((_now.add(d), c));
    return c.future;
  }

  void elapse(Duration d) {
    _now = _now.add(d);
    for (final w in [..._waits]) {
      if (!w.$1.isAfter(_now)) {
        _waits.remove(w);
        w.$2.complete();
      }
    }
  }
}

AskMsg _ask(String id, {String title = 'Let Clax read Groceries?'}) => AskMsg(
  id: id,
  kind: AskKind.crossRead,
  title: title,
  body: 'Dashboard wants to read Groceries’ data.',
  allow: 'Allow once',
  deny: 'Don’t allow',
  always: 'Always for this app',
  armMs: 500,
);

/// The answers sent to the shell.
List<AskAnswer> _answers(FakeAppSurface s) =>
    s.calls.whereType<AskAnswer>().toList();

void main() {
  late FakeAppSurface surface;
  late _Clock clock;

  Future<void> pumpScreen(WidgetTester tester) async {
    tester.view.physicalSize = const Size(1080, 2340);
    tester.view.devicePixelRatio = 3;
    addTearDown(tester.view.reset);
    final api = FakeClaxApi();
    final aid = api.seedArtifact('Dashboard', const {
      'index.html': '<!doctype html><h1>Dashboard</h1>',
    });
    surface = FakeAppSurface();
    clock = _Clock();
    final worker = StreamController<WorkerEvent>.broadcast();
    addTearDown(worker.close);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, TargetPlatform.android),
        home: AppScreen(
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: NoTimings(),
          worker: worker.stream,
          clock: clock,
        ),
      ),
    );
    surface.push(
      Ready(artifact: aid, version: 1, latest: 1, file: 'index.html'),
    );
    await tester.pump();
  }

  /// Moves the clock past [ms] and pumps the frames that follow.
  Future<void> elapse(WidgetTester tester, int ms) async {
    clock.elapse(Duration(milliseconds: ms));
    await tester.pump();
    await tester.pump();
  }

  test('the app declares the permission prompt native', () {
    expect(nativeOverlays, contains('ask'));
  });

  testWidgets('an ask shows the prompt, and Allow once armed answers it', (
    tester,
  ) async {
    await pumpScreen(tester);
    surface.push(_ask('a1'));
    await tester.pumpAndSettle();
    expect(find.text('Let Clax read Groceries?'), findsOneWidget);
    await elapse(tester, 600);
    await tester.tap(find.text('Allow once'));
    await tester.pumpAndSettle();
    expect(find.text('Let Clax read Groceries?'), findsNothing);
    expect(_answers(surface).map((a) => (a.id, a.answer)), [
      ('a1', AskAnswerValue.allow),
    ]);
  });

  testWidgets('a tap outside answers dismiss', (tester) async {
    await pumpScreen(tester);
    surface.push(_ask('a1'));
    await tester.pumpAndSettle();
    await tester.tapAt(const Offset(4, 4));
    await tester.pumpAndSettle();
    expect(_answers(surface).map((a) => (a.id, a.answer)), [
      ('a1', AskAnswerValue.dismiss),
    ]);
  });

  testWidgets('askCancel takes the prompt down and answers nothing', (
    tester,
  ) async {
    await pumpScreen(tester);
    surface.push(_ask('a1'));
    await tester.pumpAndSettle();
    surface.push(const AskCancelMsg('a1'));
    await tester.pumpAndSettle();
    expect(find.text('Let Clax read Groceries?'), findsNothing);
    expect(_answers(surface), isEmpty);
  });

  testWidgets('a second prompt waits for the first, and one cancelled while '
      'it waits is passed over', (tester) async {
    await pumpScreen(tester);
    surface
      ..push(_ask('a1'))
      ..push(_ask('a2', title: 'Download report.csv?'))
      ..push(_ask('a3', title: 'Use the microphone?'));
    await tester.pumpAndSettle();
    expect(find.text('Let Clax read Groceries?'), findsOneWidget);
    expect(find.text('Download report.csv?'), findsNothing);
    surface.push(const AskCancelMsg('a2'));
    await tester.pump();
    await tester.tap(find.text('Don’t allow'));
    await tester.pumpAndSettle();
    expect(find.text('Download report.csv?'), findsNothing);
    expect(find.text('Use the microphone?'), findsOneWidget);
    await tester.tap(find.text('Don’t allow'));
    await tester.pumpAndSettle();
    expect(_answers(surface).map((a) => (a.id, a.answer)), [
      ('a1', AskAnswerValue.deny),
      ('a3', AskAnswerValue.deny),
    ]);
  });
}
