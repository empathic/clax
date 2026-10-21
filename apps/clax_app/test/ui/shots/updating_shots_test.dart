// The cover an app's screen shows while an update runs on it, rendered into
// CLAX_SHOTS_DIR (skipped without it) on a phone, light and dark:
// `updating-<ms>` is the whole screen that long after the update began, and
// `updating-finish-<ms>` that long into its finish (the page at 440);
// `synapse-<frame>` is the synapse alone, large, every 40 ms through its
// start, a stream of dots and a finish, for stitching into a strip or a GIF.
import 'dart:math' as math;

import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/updating_cover.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

const _times = [0, 160, 420, 560, 700, 1150, 2400, 3500, 6500];
const _finish = [0, 120, 240, 340, 440];

Widget _cover(ValueNotifier<bool> updating) => ValueListenableBuilder<bool>(
  valueListenable: updating,
  builder: (_, on, _) => UpdatingCover(
    updating: on,
    title: 'Plant Diary',
    seed: 3,
    page: (_) => const Scaffold(body: Center(child: Text('The page'))),
  ),
);

/// Pumps frames 20 ms apart for [ms].
Future<void> _run(WidgetTester tester, int ms) async {
  for (var t = 0; t < ms; t += 20) {
    await tester.pump(Duration(milliseconds: math.min(20, ms - t)));
  }
}

void main() {
  for (final ms in _times) {
    shots('updating-$ms', (s) async {
      final updating = ValueNotifier(true);
      await s.tester.pumpWidget(s.app(_cover(updating)));
      await s.tester.pump();
      await _run(s.tester, ms);
      await s.take('updating-$ms');
    }, forms: [phone]);
  }
  for (final ms in _finish) {
    shots('updating-finish-$ms', (s) async {
      final updating = ValueNotifier(true);
      await s.tester.pumpWidget(s.app(_cover(updating)));
      await s.tester.pump();
      await _run(s.tester, 1500);
      updating.value = false;
      await s.tester.pump();
      await _run(s.tester, ms);
      await s.take('updating-finish-$ms');
    }, forms: [phone]);
  }
  shots('synapse', (s) async {
    const key = Key('synapse');
    final finishing = ValueNotifier(false);
    await s.tester.pumpWidget(
      s.app(
        Scaffold(
          body: Center(
            child: RepaintBoundary(
              key: key,
              child: Builder(
                builder: (context) => ColoredBox(
                  color: claxPalette(context).bg,
                  child: Padding(
                    padding: const EdgeInsets.all(24),
                    child: ValueListenableBuilder<bool>(
                      valueListenable: finishing,
                      builder: (_, f, _) =>
                          ClaxSynapse(size: 320, seed: 3, finishing: f),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
        chrome: false,
      ),
    );
    await s.tester.pump();
    const step = Duration(milliseconds: 40);
    const frames = 110;
    for (var i = 0; i < frames; i++) {
      if (i == frames - 14) finishing.value = true;
      await s.take(
        'synapse-${i.toString().padLeft(3, '0')}',
        of: find.byKey(key),
      );
      await s.tester.pump(step);
    }
  }, forms: [phone]);
}
