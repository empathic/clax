// The making screen rendered in every form and theme into CLAX_SHOTS_DIR
// (skipped without it): starting, the page streaming into the live
// preview, waiting for Claude, waiting long enough for the haiku, a
// failure with Developer tools' Details open, a new app whose starting
// data was refused, and a make stopped at a limit, with its Details open.
import 'dart:async';

import 'package:clax_app/agent/loop.dart' show MakeStopped;
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/making_preview.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

const _request = 'A tide table for the harbour and the bay at dawn';

/// The test's fake time, which `tester.pump` moves.
class _TestClock implements Clock {
  _TestClock(this.tester);

  final WidgetTester tester;

  @override
  DateTime now() => tester.binding.clock.now();

  @override
  Future<void> delay(Duration d) => Future<void>.delayed(d);
}

/// A preview that draws a stand-in page once it has loaded some text.
class _Page implements PreviewView {
  final _loaded = ValueNotifier(false);

  @override
  Future<void> loadHtml(String html) async => _loaded.value = true;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: _loaded,
    builder: (context, loaded, _) => loaded ? const _TidePage() : const SizedBox(),
  );

  @override
  void dispose() {}
}

/// A tide table as the page might draw it.
class _TidePage extends StatelessWidget {
  const _TidePage();

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    final t = Theme.of(context).textTheme;
    return ColoredBox(
      color: p.bg,
      child: Padding(
        padding: const EdgeInsets.all(20),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Harbour tides', style: t.titleLarge),
            const SizedBox(height: 4),
            Text('Sunday 19 October', style: t.bodyMedium?.copyWith(color: p.muted)),
            const SizedBox(height: 16),
            for (final (time, kind, m) in const [
              ('04:12', 'High', '4.1 m'),
              ('10:31', 'Low', '0.6 m'),
              ('16:40', 'High', '4.3 m'),
              ('22:58', 'Low', '0.5 m'),
            ])
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 6),
                child: Row(
                  children: [
                    SizedBox(width: 64, child: Text(time, style: t.bodyLarge)),
                    Expanded(child: Text(kind, style: t.bodyLarge)),
                    Text(m, style: t.bodyLarge?.copyWith(color: p.muted)),
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// Pumps the making screen of [_request] in [s]'s window; [report] is set
/// once the make starts, [result] ends it.
Future<void Function(MakeUpdate)> _pump(
  Shot s,
  Completer<String> result, {
  bool developer = false,
}) async {
  late void Function(MakeUpdate) report;
  final on = ValueNotifier(developer);
  addTearDown(on.dispose);
  await s.tester.pumpWidget(
    DeveloperScope(
      notifier: on,
      child: s.app(
        MakingScreen(
          request: _request,
          make: (_, _, r) {
            report = r;
            return result.future;
          },
          keepGoing: (_, _, _, _) => Completer<String>().future,
          preview: _Page.new,
          clock: _TestClock(s.tester),
          haikus: () async => const [
            'Low tide, the boats lean —\nthe harbour holds its breath\nuntil the sea comes',
          ],
        ),
        title: 'New app',
      ),
    ),
  );
  await s.tester.pump();
  return report;
}

void main() {
  shots('making-start', (s) async {
    final done = Completer<String>();
    await _pump(s, done);
    await s.settle();
    await s.take('making-start');
    done.complete('a1');
    await s.tester.pump();
  });

  shots('making-live', (s) async {
    final done = Completer<String>();
    final report = await _pump(s, done);
    void p(AgentProgress e) => report(MakeProgress(e));
    report(MakeStage.writing);
    p(const Narration('Laying out the day’s tides.'));
    p(toolStarted('write_file', {'path': 'index.html'}));
    p(const PartialFile('index.html', '<h1>Harbour tides</h1>'));
    await s.settle();
    await s.take('making-live');
    done.complete('a1');
    await s.tester.pump();
  });

  shots('making-waiting', (s) async {
    final done = Completer<String>();
    final report = await _pump(s, done);
    report(MakeStage.writing);
    report(const MakeProgress(Narration('Laying out the day’s tides.')));
    report(MakeStage.offline);
    await s.settle();
    await s.take('making-waiting');
    done.complete('a1');
    await s.tester.pump();
  });

  shots('making-haiku', (s) async {
    final done = Completer<String>();
    final report = await _pump(s, done);
    void p(AgentProgress e) => report(MakeProgress(e));
    report(MakeStage.writing);
    p(const Narration('Laying out the day’s tides.'));
    p(toolStarted('write_file', {'path': 'index.html'}));
    p(const PartialFile('index.html', '<h1>Harbour tides</h1>'));
    p(toolStarted('render', {}));
    await s.tester.pump(const Duration(seconds: 9));
    await s.settle();
    await s.take('making-haiku');
    done.complete('a1');
    await s.tester.pump();
  });

  shots('making-failed-details', (s) async {
    final done = Completer<String>();
    await _pump(s, done, developer: true);
    done.completeError(
      StateError('HTTP 529 from api.anthropic.com: overloaded_error'),
    );
    await s.settle();
    await s.tester.tap(find.byKey(const ValueKey('make-failure-details')));
    await s.settle();
    await s.take('making-failed-details');
  });

  shots('making-refused', (s) async {
    final done = Completer<String>();
    final report = await _pump(s, done);
    void p(AgentProgress e) => report(MakeProgress(e));
    p(const PartialFile('index.html', '<h1>Harbour tides</h1>'));
    p(const StartingDataRefused('a1', 'quota_exceeded: full'));
    done.complete('a1');
    await s.settle();
    await s.take('making-refused');
  });

  shots('making-stopped', (s) async {
    final done = Completer<String>();
    await _pump(s, done);
    done.completeError(
      const MakeStopped(
        key: 'new:k1',
        limit: '14 turns',
        stops: 1,
        spentUsd: 1.18,
        notes: [
          'Planning the tide table',
          'Writing the day view',
          'I wrote the day view and the tide list, but ran out of room '
              'before I could get a clean render.',
        ],
        changed: [
          (path: 'app.js', added: 212, removed: 0),
          (path: 'index.html', added: 48, removed: 0),
        ],
        html: '<h1>Harbour tides</h1>',
      ),
    );
    await s.settle();
    await s.take('making-stopped');
    final details = find.byKey(const ValueKey('make-stop-details'));
    await s.tester.ensureVisible(details);
    await s.tester.tap(details);
    await s.settle();
    await s.take('making-stopped-details');
  });
}
