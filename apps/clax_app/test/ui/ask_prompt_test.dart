import 'dart:async';

import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/ui/ask_prompt.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/kit/toggles.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// A clock that moves only on [elapse], finishing the delays it reaches.
class _Clock implements Clock {
  final _waits = <(Duration, Completer<void>)>[];
  Duration _now = Duration.zero;

  @override
  DateTime now() => DateTime.fromMillisecondsSinceEpoch(_now.inMilliseconds);

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _waits.add((_now + d, c));
    return c.future;
  }

  void elapse(Duration d) {
    _now += d;
    for (final w in [..._waits]) {
      if (w.$1 <= _now) {
        _waits.remove(w);
        w.$2.complete();
      }
    }
  }
}

const _msg = AskMsg(
  id: 'a1',
  kind: AskKind.crossRead,
  title: 'Let Clax read Groceries?',
  body: 'Dashboard wants to read Groceries’ data.',
  allow: 'Allow once',
  deny: 'Don’t allow',
  always: 'Always for this app',
  armMs: 500,
);

/// A prompt under test: [answer] once answered.
class _Prompt {
  _Prompt(this.tester, this.clock);
  final WidgetTester tester;
  final _Clock clock;
  AskAnswerValue? answer;
  final cancel = Completer<void>();

  /// Shows [m] on [platform]; the clock reads 0 at its first frame.
  Future<void> show({
    AskMsg m = _msg,
    TargetPlatform platform = TargetPlatform.android,
  }) async {
    tester.view.physicalSize = const Size(412, 915);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, platform),
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: TextButton(
                onPressed: () async => answer = await AskPrompt.show(
                  context,
                  m,
                  clock: clock,
                  cancelled: cancel.future,
                ),
                child: const Text('Go'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Go'));
    await tester.pumpAndSettle();
  }

  /// Moves the clock to [ms] after the first frame, and pumps the frames
  /// that follow (arming lands in a microtask, then rebuilds).
  Future<void> at(int ms) async {
    final now = clock.now().millisecondsSinceEpoch;
    clock.elapse(Duration(milliseconds: ms - now));
    await tester.pump();
    await tester.pump();
  }
}

Finder _button(String label) => find.ancestor(
  of: find.text(label),
  matching: find.byWidgetPredicate((w) => w is ButtonStyleButton),
);

bool _enabled(WidgetTester tester, String label) =>
    tester.widget<ButtonStyleButton>(_button(label)).onPressed != null;

/// Whether "Allow in a moment…" shows.
bool _waitingShown(WidgetTester tester) => tester
    .widget<Visibility>(
      find.ancestor(
        of: find.text('Allow in a moment…'),
        matching: find.byType(Visibility),
      ),
    )
    .visible;

void main() {
  for (final platform in [TargetPlatform.android, TargetPlatform.macOS]) {
    group('on $platform', () {
      testWidgets('Allow is disabled with its waiting line until it arms', (
        tester,
      ) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        expect(find.text('Let Clax read Groceries?'), findsOneWidget);
        expect(_enabled(tester, 'Allow once'), isFalse);
        expect(_waitingShown(tester), isTrue);
        await p.at(499);
        expect(_enabled(tester, 'Allow once'), isFalse);
        expect(_waitingShown(tester), isTrue);
        await p.at(500);
        expect(_enabled(tester, 'Allow once'), isTrue);
        // The line keeps its room, hidden, so the buttons stay put.
        expect(_waitingShown(tester), isFalse);
      });

      testWidgets('its first frame focuses Don’t allow', (tester) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await tester.pump();
        final focus = Focus.of(tester.element(find.text('Don’t allow')));
        expect(focus.hasPrimaryFocus, isTrue);
      });

      testWidgets('a press that began before arming answers nothing', (
        tester,
      ) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await p.at(499);
        final g = await tester.startGesture(
          tester.getCenter(find.text('Allow once')),
        );
        await p.at(520);
        await g.up();
        await tester.pumpAndSettle();
        expect(p.answer, isNull);
        expect(find.text('Let Clax read Groceries?'), findsOneWidget);
      });

      testWidgets('a tap after arming answers allow', (tester) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await p.at(600);
        await tester.tap(find.text('Allow once'));
        await tester.pumpAndSettle();
        expect(p.answer, AskAnswerValue.allow);
      });

      testWidgets('a Return held from before arming answers nothing', (
        tester,
      ) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await p.at(400);
        await tester.sendKeyDownEvent(LogicalKeyboardKey.enter);
        await p.at(600);
        await tester.sendKeyRepeatEvent(LogicalKeyboardKey.enter);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.enter);
        await tester.pumpAndSettle();
        expect(p.answer, isNull);
        expect(find.text('Let Clax read Groceries?'), findsOneWidget);
      });

      testWidgets('a Return that went down after arming answers allow', (
        tester,
      ) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await p.at(501);
        await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        await tester.pumpAndSettle();
        expect(p.answer, AskAnswerValue.allow);
      });

      testWidgets('Space on Allow, focused after arming, answers allow', (
        tester,
      ) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await p.at(600);
        // Tab until Allow has focus (the order depends on whether the
        // buttons fit side by side).
        bool onAllow() =>
            Focus.of(tester.element(find.text('Allow once'))).hasPrimaryFocus;
        for (var i = 0; i < 3 && !onAllow(); i++) {
          await tester.sendKeyEvent(LogicalKeyboardKey.tab);
          await tester.pump();
        }
        expect(onAllow(), isTrue);
        await tester.sendKeyEvent(LogicalKeyboardKey.space);
        await tester.pumpAndSettle();
        expect(p.answer, AskAnswerValue.allow);
      });

      testWidgets('Space on Don’t allow answers deny', (tester) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await tester.sendKeyEvent(LogicalKeyboardKey.space);
        await tester.pumpAndSettle();
        expect(p.answer, AskAnswerValue.deny);
      });

      testWidgets('Escape answers dismiss', (tester) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await tester.pumpAndSettle();
        expect(p.answer, AskAnswerValue.dismiss);
      });

      testWidgets('a tap outside answers dismiss', (tester) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await tester.tapAt(const Offset(4, 4));
        await tester.pumpAndSettle();
        expect(p.answer, AskAnswerValue.dismiss);
      });

      testWidgets('Don’t allow answers deny, before arming too', (
        tester,
      ) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        await tester.tap(find.text('Don’t allow'));
        await tester.pumpAndSettle();
        expect(p.answer, AskAnswerValue.deny);
      });

      testWidgets('with Always checked, Allow answers always', (tester) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        expect(find.byType(ClaxCheckbox), findsOneWidget);
        await tester.tap(find.text('Always for this app'));
        await p.at(600);
        await tester.tap(find.text('Allow once'));
        await tester.pumpAndSettle();
        expect(p.answer, AskAnswerValue.always);
      });

      testWidgets('without an always label there is no checkbox', (
        tester,
      ) async {
        final p = _Prompt(tester, _Clock());
        await p.show(
          platform: platform,
          m: const AskMsg(
            id: 'a2',
            kind: AskKind.download,
            title: 'Download report.csv?',
            body: '',
            allow: 'Allow',
            deny: 'Don’t allow',
            armMs: 500,
          ),
        );
        expect(find.byType(ClaxCheckbox), findsNothing);
      });

      testWidgets('cancelled, it goes and answers dismiss', (tester) async {
        final p = _Prompt(tester, _Clock());
        await p.show(platform: platform);
        p.cancel.complete();
        await tester.pumpAndSettle();
        expect(find.text('Let Clax read Groceries?'), findsNothing);
        expect(p.answer, AskAnswerValue.dismiss);
      });
    });
  }
}
