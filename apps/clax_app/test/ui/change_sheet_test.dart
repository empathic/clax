import 'dart:convert';

import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/ui/change_sheet.dart';
import 'package:clax_app/ui/change_tracker.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// A 1 x 1 PNG.
final _png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
);

/// The change sheet over a page of [size] on [platform], for a change
/// [tracker] follows on a thread of its own.
Future<void> _show(
  WidgetTester tester,
  ChangeTracker Function(String tid) tracker, {
  Size size = const Size(412, 915),
  TargetPlatform platform = TargetPlatform.android,
  VoidCallback? onStop,
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final api = FakeClaxApi();
  final aid = api.seedArtifact('Focus', {'index.html': '<p>x</p>'});
  final tid = api.sendComment(aid, 'Show this week').threadId;
  final changes = tracker(tid);
  await tester.pumpWidget(
    MaterialApp(
      key: UniqueKey(),
      theme: claxTheme(Brightness.light, platform),
      home: Builder(
        builder: (context) => Scaffold(
          body: Center(
            child: TextButton(
              onPressed: () => showChangeSheet(
                context,
                changes: changes,
                updates: ValueNotifier(0),
                clock: const SystemClock(),
                api: api,
                aid: aid,
                onThread: (_, _, {reply = false}) {},
                onStop: onStop,
                onRetry: () {},
              ),
              child: const Text('show'),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('show'));
  // The spinner never settles: pump through the sheet's entrance.
  for (var i = 0; i < 6; i++) {
    await tester.pump(const Duration(milliseconds: 100));
  }
}

ChangeTracker _working(String tid, {int errors = 0}) =>
    ChangeTracker(timings: const NoTimings(), artifact: 'a')
      ..sent(tid)
      ..workerWorking([tid])
      ..workerProgress([tid], const Narration('Reading the page'))
      ..workerProgress([tid], Inspected(_png, errors));

void main() {
  testWidgets('on a phone it has a handle and no close button', (tester) async {
    await _show(tester, _working);
    expect(find.byKey(const ValueKey('change-sheet')), findsOneWidget);
    expect(
      tester.widget<BottomSheet>(find.byType(BottomSheet)).showDragHandle,
      isTrue,
    );
    expect(find.byKey(const ValueKey('change-close')), findsNothing);
  });

  testWidgets('wide it is a side panel with a close button and no handle', (
    tester,
  ) async {
    await _show(tester, _working, size: const Size(1280, 800));
    expect(find.byType(BottomSheet), findsNothing);
    expect(find.byKey(const ValueKey('change-close')), findsOneWidget);
  });

  testWidgets('a clean render says nothing of errors; one with errors '
      'counts them', (tester) async {
    await _show(tester, _working);
    expect(find.byKey(const ValueKey('change-render')), findsOneWidget);
    expect(find.text('No errors'), findsNothing);
    expect(find.byKey(const ValueKey('change-render-errors')), findsNothing);
    await _show(tester, (tid) => _working(tid, errors: 2));
    expect(find.text('2 errors'), findsOneWidget);
  });

  testWidgets('Stop is destructive and Open thread tonal, both 40 high', (
    tester,
  ) async {
    await _show(tester, _working, onStop: () {});
    final stop = find.byKey(const ValueKey('change-stop'));
    final open = find.byKey(const ValueKey('change-open-thread'));
    expect(tester.widget<ClaxButton>(stop).kind, ClaxButtonKind.destructive);
    expect(tester.widget<ClaxButton>(open).kind, ClaxButtonKind.tonal);
    for (final b in [stop, open]) {
      final painted = find.descendant(of: b, matching: find.byType(Material));
      expect(tester.getSize(painted.first).height, 40);
    }
  });

  testWidgets('a limit stop says Keep going in the pointer’s words', (
    tester,
  ) async {
    ChangeTracker stopped(String tid) =>
        ChangeTracker(timings: const NoTimings(), artifact: 'a')
          ..sent(tid)
          ..workerWorking([tid])
          ..workerFailed(limitFailureReason('10 turns', leftUsd: 6));
    await _show(tester, stopped);
    expect(find.textContaining(keepGoingHint), findsOneWidget);
    await _show(
      tester,
      stopped,
      size: const Size(1200, 800),
      platform: TargetPlatform.macOS,
    );
    expect(
      find.textContaining(keepGoingHintFor(ClaxVerbs.mouse)),
      findsOneWidget,
    );
    expect(find.textContaining('Tap Keep going'), findsNothing);
  });

  group('after the change ends', () {
    ChangeTracker failed(String tid, String reason) =>
        ChangeTracker(timings: const NoTimings(), artifact: 'a')
          ..sent(tid)
          ..workerWorking([tid])
          ..workerFailed(reason);
    final ends = <String, ChangeTracker Function(String)>{
      'failed': (tid) => failed(tid, 'the connection dropped'),
      'limit-stopped': (tid) =>
          failed(tid, limitFailureReason('10 turns', leftUsd: 6)),
      'spent': (tid) => failed(tid, limitFailureReason('10 turns', leftUsd: 0)),
      'stopped': (tid) =>
          ChangeTracker(timings: const NoTimings(), artifact: 'a')
            ..sent(tid)
            ..workerWorking([tid])
            ..stopped(),
    };
    for (final MapEntry(key: name, value: end) in ends.entries) {
      testWidgets('a $name change shows what was asked and Open thread', (
        tester,
      ) async {
        await _show(tester, end);
        await tester.pump();
        expect(find.text('Waiting for the comment…'), findsNothing);
        expect(find.textContaining('Show this week'), findsOneWidget);
        expect(
          find.byKey(const ValueKey('change-open-thread')),
          findsOneWidget,
        );
      });
    }

    testWidgets('a failure says “Couldn’t make that change” once', (
      tester,
    ) async {
      await _show(
        tester,
        (tid) => failed(tid, limitFailureReason('10 turns', leftUsd: 6)),
      );
      expect(find.textContaining('Couldn’t make that change'), findsOneWidget);
      final line = tester.widget<Text>(
        find.byKey(const ValueKey('change-step')),
      );
      expect(line.data, startsWith('It was too big for one go'));
    });

    testWidgets('a limit stop that spent the change’s limit offers Done, '
        'not Retry', (tester) async {
      await _show(tester, ends['spent']!);
      expect(find.byKey(const ValueKey('change-retry')), findsNothing);
      expect(find.byKey(const ValueKey('change-done')), findsOneWidget);
    });
  });
}
