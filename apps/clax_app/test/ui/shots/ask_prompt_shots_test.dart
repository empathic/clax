// The native permission prompt rendered into CLAX_SHOTS_DIR (skipped
// without it): before Allow arms, with its waiting line, and after. The
// Apple family is drawn on the iPhone; the Mac shows an NSAlert sheet
// (NativeDialogs.alert), which flutter_test cannot draw.
import 'dart:async';

import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/ui/ask_prompt.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

/// A clock that moves only on [elapse].
class _Clock implements Clock {
  final _waits = <(Duration, Completer<void>)>[];
  var _now = Duration.zero;

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
  body: 'Dashboard wants to read Groceries’ data. Clax never changes it.',
  allow: 'Allow once',
  deny: 'Don’t allow',
  always: 'Always for this app',
  armMs: 500,
);

/// The prompt over a page, armed or not, and the shot.
Future<void> _prompt(Shot s, String surface, {required bool armed}) async {
  final nav = GlobalKey<NavigatorState>();
  final clock = _Clock();
  await s.tester.pumpWidget(
    s.app(
      Navigator(
        key: nav,
        onGenerateRoute: (_) => MaterialPageRoute<void>(
          builder: (_) => Scaffold(
            appBar: AppBar(title: const Text('Dashboard')),
            body: const Center(child: Text('A page')),
          ),
        ),
      ),
    ),
  );
  Future<AskAnswerValue> _ = AskPrompt.show(
    nav.currentContext!,
    _msg,
    clock: clock,
  );
  await s.settle();
  if (armed) {
    clock.elapse(const Duration(milliseconds: 500));
    await s.settle();
  }
  expect(find.text(_msg.title), findsOneWidget);
  await s.take(surface);
}

void main() {
  shots(
    'ask-prompt',
    (s) => _prompt(s, 'ask-prompt', armed: false),
    forms: const [phone, wide, iphone],
  );
  shots(
    'ask-prompt-armed',
    (s) => _prompt(s, 'ask-prompt-armed', armed: true),
    forms: const [phone, wide, iphone],
  );
  shots(
    'ask-prompt',
    (s) => _prompt(s, 'ask-prompt', armed: false),
    forms: const [phone],
    textScales: const [2],
  );
}
