// The change sheet rendered in every form and theme into CLAX_SHOTS_DIR
// (skipped without it): a change in progress with its steps and a render,
// and a change stopped at a limit, offering Keep going.
import 'dart:convert';

import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/ui/change_sheet.dart';
import 'package:clax_app/ui/change_tracker.dart';
import 'package:flutter/material.dart';

import 'rig.dart';

/// A 1 x 1 PNG, for the render.
final _png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
);

Future<void> _change(
  Shot s,
  String surface,
  void Function(ChangeTracker t, String tid) state,
) async {
  final api = FakeClaxApi();
  final aid = api.seedArtifact('Focus', {'index.html': '<p>x</p>'});
  final tid = api
      .sendComment(
        aid,
        'Make the session log show this week, with a total per day',
        author: 'Alex',
      )
      .threadId;
  final changes = ChangeTracker(timings: const NoTimings(), artifact: aid);
  state(changes, tid);
  final page = GlobalKey();
  await s.tester.pumpWidget(
    s.app(
      Scaffold(
        key: page,
        appBar: AppBar(title: const Text('Focus')),
        body: const Center(child: Text('A page')),
      ),
    ),
  );
  showChangeSheet(
    page.currentContext!,
    changes: changes,
    updates: ValueNotifier(0),
    clock: const SystemClock(),
    api: api,
    aid: aid,
    onThread: (_, _, {reply = false}) {},
    onStop: () {},
    onRetry: () {},
  );
  await s.settle();
  await s.take(surface);
}

void main() {
  shots(
    'change-sheet',
    (s) => _change(s, 'change-sheet', (t, tid) {
      t
        ..sent(tid)
        ..workerWorking([tid])
        ..workerProgress([tid], const Narration('Reading the session log'))
        ..workerProgress([tid], const Narration('Adding a total per day'))
        ..workerProgress([tid], Inspected(_png, 0));
    }),
  );
  shots(
    'change-sheet-limit',
    (s) => _change(s, 'change-sheet-limit', (t, tid) {
      t
        ..sent(tid)
        ..workerWorking([tid])
        ..workerFailed(limitFailureReason('10 turns', leftUsd: 6));
    }),
  );
}
