// Waiting in tests for work that runs real IO: on a condition the test
// names, never on a count of pumps or event-loop turns, which run out
// first under load.
import 'dart:async';

import 'package:flutter_test/flutter_test.dart';

/// How long a wait may run before the test is taken as stuck. Generous: it
/// only ends a test whose condition will never hold.
const stuckAfter = Duration(minutes: 2);

/// Turns the real event loop (so file IO, crypto and the services' other
/// real work run) and pumps 50 ms frames, until [until] finds a widget;
/// then pumps the sheets through their transitions.
Future<void> settle(WidgetTester tester, Finder until) => settleUntil(
  tester,
  () => until.evaluate().isNotEmpty,
  what: until.toString(describeSelf: true),
);

/// [settle] until [until] holds; an [until] that answers a future is
/// awaited on the real event loop.
Future<void> settleUntil(
  WidgetTester tester,
  FutureOr<bool> Function() until, {
  String what = 'the condition',
}) async {
  final watch = Stopwatch()..start();
  while (true) {
    await tester.runAsync(() => Future<void>.delayed(Duration.zero));
    await tester.pump(const Duration(milliseconds: 50));
    if (await tester.runAsync(() async => until()) ?? false) break;
    if (watch.elapsed > stuckAfter) {
      fail('Waited $stuckAfter for $what');
    }
  }
  for (var i = 0; i < 8; i++) {
    await tester.pump(const Duration(milliseconds: 50));
  }
}

/// Turns the real event loop until [until] holds, outside a widget test.
Future<void> waitUntil(
  FutureOr<bool> Function() until, {
  String what = 'the condition',
}) async {
  final watch = Stopwatch()..start();
  while (!await until()) {
    if (watch.elapsed > stuckAfter) fail('Waited $stuckAfter for $what');
    await Future<void>.delayed(Duration.zero);
  }
}
