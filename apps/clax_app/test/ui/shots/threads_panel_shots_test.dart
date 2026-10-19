// The threads panel over an app, rendered in every form and theme into
// CLAX_SHOTS_DIR (skipped without it): open threads titled by what they are
// on, one with a wide screenshot on its own line and one with a narrow one
// beside its words, an unsent one with its Send to Clax button, and the
// resolved threads collapsed (`threads-panel`) and open
// (`threads-panel-resolved`).
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/threads_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

/// A [width] x [height] PNG: a row of a page, bars of text on a card.
Future<Uint8List> _clip(int width, int height) async {
  final rec = ui.PictureRecorder();
  final c = ui.Canvas(rec);
  final w = width.toDouble(), h = height.toDouble();
  c.drawRect(Rect.fromLTWH(0, 0, w, h), Paint()..color = const Color(0xFFF7F4EE));
  final ink = Paint()..color = const Color(0xFF2B2A27);
  final soft = Paint()..color = const Color(0xFFB9B3A8);
  for (var y = h * 0.18, i = 0; y < h * 0.85; y += h * 0.22, i++) {
    c.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromLTWH(w * 0.04, y, w * (i.isEven ? 0.30 : 0.22), h * 0.1),
        const Radius.circular(4),
      ),
      ink,
    );
    c.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromLTWH(w * 0.40, y, w * 0.5, h * 0.1),
        const Radius.circular(4),
      ),
      soft,
    );
  }
  final image = await rec.endRecording().toImage(width, height);
  final data = await image.toByteData(format: ui.ImageByteFormat.png);
  image.dispose();
  return data!.buffer.asUint8List();
}

const _threads = [
  ThreadSummary(
    id: 'r1',
    status: 'resolved',
    summary: 'Use kilometres, not miles',
    sent: true,
    comments: 2,
    version: 1,
    label: '“Distance”',
  ),
  ThreadSummary(
    id: 'r2',
    status: 'resolved',
    summary: 'The title is cut off on my phone',
    sent: true,
    comments: 4,
    version: 2,
    label: 'Heading',
  ),
  ThreadSummary(
    id: 'n',
    status: 'open',
    summary: 'Make this a bar chart, one bar per window',
    sent: true,
    comments: 3,
    version: 3,
    label: 'Chart under “p95 by four-hour window”',
    clip: true,
  ),
  ThreadSummary(
    id: 'a',
    status: 'open',
    summary: 'Warmer colours everywhere, please',
    sent: true,
    comments: 1,
    version: 3,
    label: 'Whole app',
  ),
  ThreadSummary(
    id: 'w',
    status: 'open',
    summary: 'Show the pace next to each run and bold the longest one',
    sent: false,
    comments: 1,
    version: 3,
    label: '“Mon 5 km Tue 8 km Thu 12 km”',
    clip: true,
  ),
];

void main() {
  for (final open in [false, true]) {
    shots(open ? 'threads-panel-resolved' : 'threads-panel', (s) async {
      final clips = (await s.tester.runAsync(
        () async => {'w': await _clip(1600, 400), 'n': await _clip(800, 600)},
      ))!;
      final sideways = s.form != phone;
      final panel = AppPanel(
        wide: sideways,
        tab: PanelTab.threads,
        onTab: (_) {},
        onClose: () {},
        threads: _threads,
        onThread: (_) {},
        onResend: (_) {},
        versions: const SizedBox(),
        clipOf: (id) async => clips[id],
      );
      await s.tester.pumpWidget(
        s.app(
          Builder(
            builder: (context) => Scaffold(
              backgroundColor: Theme.of(context).colorScheme.surface,
              body: sideways
                  ? Row(
                      children: [
                        const Expanded(child: SizedBox()),
                        SizedBox(width: 360, child: panel),
                      ],
                    )
                  : Column(
                      children: [
                        const Expanded(flex: 2, child: SizedBox()),
                        Expanded(flex: 5, child: panel),
                      ],
                    ),
            ),
          ),
        ),
      );
      await s.settle();
      if (open) {
        await s.tester.tap(find.text('Resolved (2)'));
        await s.settle();
      }
      await s.take(open ? 'threads-panel-resolved' : 'threads-panel');
    });
  }
}
